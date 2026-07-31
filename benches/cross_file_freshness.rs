//! Cross-file diagnostic-freshness wall-clock scenario (P0.3 headline).
//!
//! Drives the real LSP server against a synthetic workspace where every file
//! extends the edited base class — the worst case for any dependent-sweep
//! architecture — and measures the wall time users actually feel: from a
//! keystroke in `base.php` to the *dependent* open file's diagnostics
//! reflecting the change (error appears on rename, clears on revert), without
//! the dependent ever being touched.
//!
//! WS3 property under test: that latency stays flat as the ingested-file
//! count grows 100→5000, because the republish path re-analyzes only the open
//! files and salsa memoization absorbs the rest. Gate: absolute ceiling on
//! the p50 per size (an O(N) regression at 5000 files lands in seconds).
//!
//! Run with `cargo bench --bench cross_file_freshness`. Release mode matters.

use php_lsp::backend::Backend;
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tower_lsp_server::{LspService, Server};

const SIZES: &[usize] = &[100, 1000, 5000];
const CYCLES: usize = 8;
const WARMUP_CYCLES: usize = 2;
/// Edit → dependent-diagnostics ceiling. Debounce (~100 ms) + two open-file
/// analyses; measured well under 300 ms at every size. The retired
/// dependent-sweep re-analyzed all N files per edit — seconds at 5000 — so
/// this ceiling trips immediately on an O(N) regression.
const FRESHNESS_CEILING_MS: f64 = 600.0;
const INDEX_READY_TIMEOUT_SECS: u64 = 300;
const DIAG_TIMEOUT_SECS: u64 = 30;

// ---------- framing ----------

fn frame(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_string(msg).unwrap();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

async fn read_msg(reader: &mut (impl AsyncReadExt + Unpin)) -> Value {
    let mut header_buf = Vec::new();
    loop {
        let b = reader.read_u8().await.expect("read byte");
        header_buf.push(b);
        if header_buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_str = std::str::from_utf8(&header_buf).unwrap();
    let content_length: usize = header_str
        .lines()
        .find(|l| l.to_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
        .expect("Content-Length header");
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await.expect("read body");
    serde_json::from_slice(&body).expect("parse JSON")
}

struct Client {
    write: WriteHalf<DuplexStream>,
    read: ReadHalf<DuplexStream>,
}

impl Client {
    async fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.write.write_all(&frame(&msg)).await.unwrap();
        loop {
            let resp = read_msg(&mut self.read).await;
            if resp.get("method").is_some() {
                if let Some(srv_id) = resp.get("id") {
                    let ack = json!({"jsonrpc":"2.0","id":srv_id,"result":null});
                    self.write.write_all(&frame(&ack)).await.unwrap();
                }
                continue;
            }
            if resp.get("id") == Some(&json!(id)) {
                return resp;
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc":"2.0","method":method,"params":params});
        self.write.write_all(&frame(&msg)).await.unwrap();
    }

    async fn wait_for_index_ready(&mut self) -> bool {
        tokio::time::timeout(Duration::from_secs(INDEX_READY_TIMEOUT_SECS), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method") == Some(&json!("$/php-lsp/indexReady")) {
                    return;
                }
                if msg.get("method").is_some()
                    && let Some(id) = msg.get("id")
                {
                    let ack = json!({"jsonrpc":"2.0","id":id,"result":null});
                    self.write.write_all(&frame(&ack)).await.unwrap();
                }
            }
        })
        .await
        .is_ok()
    }

    /// Wait for a `publishDiagnostics` for `uri` whose emptiness matches
    /// `expect_error`. Skips publishes for other files and stale publishes
    /// for `uri` in the opposite state.
    async fn wait_for_diag_state(&mut self, uri: &str, expect_error: bool) -> bool {
        let uri_val = json!(uri);
        tokio::time::timeout(Duration::from_secs(DIAG_TIMEOUT_SECS), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method") == Some(&json!("textDocument/publishDiagnostics"))
                    && msg["params"]["uri"] == uri_val
                {
                    let has_error = msg["params"]["diagnostics"]
                        .as_array()
                        .is_some_and(|d| !d.is_empty());
                    if has_error == expect_error {
                        return;
                    }
                }
                if msg.get("method").is_some()
                    && let Some(id) = msg.get("id")
                {
                    let ack = json!({"jsonrpc":"2.0","id":id,"result":null});
                    self.write.write_all(&frame(&ack)).await.unwrap();
                }
            }
        })
        .await
        .is_ok()
    }
}

fn spawn_server() -> Client {
    let (client_stream, server_stream) = tokio::io::duplex(1 << 20);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (service, socket) = LspService::new(Backend::new);
    tokio::spawn(Server::new(server_read, server_write, socket).serve(service));
    Client {
        write: client_write,
        read: client_read,
    }
}

// ---------- workspace ----------

fn base_text(renamed: bool) -> String {
    let name = if renamed { "BaseRenamed" } else { "Base" };
    format!(
        "<?php\nclass {name} {{\n    public function ping(): int {{\n        return 1;\n    }}\n}}\n"
    )
}

const CONSUMER_TEXT: &str = "<?php\nclass Consumer {\n    public function go(): int {\n        $b = new Base();\n        return $b->ping();\n    }\n}\n";

fn write_workspace(root: &Path, size: usize) {
    std::fs::write(root.join("base.php"), base_text(false)).unwrap();
    std::fs::write(root.join("consumer.php"), CONSUMER_TEXT).unwrap();
    for i in 0..size {
        std::fs::write(
            root.join(format!("dep{i}.php")),
            format!("<?php\nclass Dep{i} extends Base {{\n    public function go(): int {{\n        return $this->ping() + {i};\n    }}\n}}\n"),
        )
        .unwrap();
    }
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = (p / 100.0) * (samples.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        samples[lo]
    } else {
        samples[lo] + (samples[hi] - samples[lo]) * (rank - lo as f64)
    }
}

async fn run_size(size: usize) -> Option<(f64, f64)> {
    let dir = tempfile::tempdir().expect("tempdir");
    write_workspace(dir.path(), size);
    let root_uri = format!("file://{}", dir.path().display());
    let base_uri = format!("{root_uri}/base.php");
    let consumer_uri = format!("{root_uri}/consumer.php");

    let mut c = spawn_server();
    c.request(
        1,
        "initialize",
        json!({"processId": null, "rootUri": root_uri, "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    if !c.wait_for_index_ready().await {
        eprintln!("  size {size}: indexReady timeout — skipping");
        return None;
    }

    for (uri, text) in [
        (&base_uri, base_text(false)),
        (&consumer_uri, CONSUMER_TEXT.to_string()),
    ] {
        c.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "php", "version": 1, "text": text}}),
        )
        .await;
    }
    // Drain the initial publishes: consumer must settle clean.
    if !c.wait_for_diag_state(&consumer_uri, false).await {
        eprintln!("  size {size}: consumer never settled clean — skipping");
        return None;
    }

    let mut samples: Vec<f64> = Vec::new();
    let mut version = 1i64;
    for cycle in 0..CYCLES {
        for renamed in [true, false] {
            version += 1;
            let t = Instant::now();
            c.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": base_uri, "version": version},
                    "contentChanges": [{"text": base_text(renamed)}],
                }),
            )
            .await;
            // Renaming Base breaks consumer.php (`new Base()` → UndefinedClass);
            // reverting heals it. Freshness = the dependent's publish reflecting
            // the base edit, without consumer.php being touched.
            if !c.wait_for_diag_state(&consumer_uri, renamed).await {
                eprintln!("  size {size}: dependent diagnostics never updated — abort");
                return None;
            }
            if cycle >= WARMUP_CYCLES {
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
    }

    let p50 = percentile(&mut samples, 50.0);
    let p95 = percentile(&mut samples, 95.0);
    Some((p50, p95))
}

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    println!(
        "cross-file freshness — base-class edit → dependent publishDiagnostics ({} samples/size)",
        (CYCLES - WARMUP_CYCLES) * 2
    );
    println!("{:>8}  {:>9}  {:>9}", "files", "p50 ms", "p95 ms");

    let mut worst_p50 = 0f64;
    let mut measured_all = true;
    for &size in SIZES {
        match rt.block_on(run_size(size)) {
            Some((p50, p95)) => {
                worst_p50 = worst_p50.max(p50);
                println!("{size:>8}  {p50:>9.1}  {p95:>9.1}");
            }
            None => {
                measured_all = false;
                println!("{size:>8}  {:>9}  {:>9}", "—", "—");
            }
        }
    }

    let ok = measured_all && worst_p50 <= FRESHNESS_CEILING_MS;
    println!(
        "\nworst p50 {worst_p50:.1} ms (ceiling {FRESHNESS_CEILING_MS}): {}",
        if ok {
            "OK"
        } else {
            "OVER — cross-file freshness re-coupled to workspace size!"
        }
    );
    if !ok {
        std::process::exit(1);
    }
}
