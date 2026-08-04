//! Find-references latency across every symbol kind `textDocument/references`
//! can resolve, measured through the real LSP protocol (the actual client-
//! visible path) against the real Laravel fixture as workspace noise.
//!
//! For each kind: opens the target declaration site, requests references
//! once cold, then 10 more times warm (no edits in between), reporting
//! cold/warm latency and the reference count found (a correctness check —
//! a "fast" zero-result query would be a false positive).
//!
//! Run: `cargo bench --bench references_all_kinds`

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use php_lsp::backend::Backend;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tower_lsp_server::{LspService, Server};

struct Client {
    write: WriteHalf<DuplexStream>,
    read: ReadHalf<DuplexStream>,
    next_id: i64,
}

fn frame(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_string(msg).unwrap();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

async fn read_msg(reader: &mut (impl AsyncReadExt + Unpin)) -> Value {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        reader.read_exact(&mut byte).await.unwrap();
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_str = String::from_utf8_lossy(&header);
    let len: usize = header_str
        .lines()
        .find_map(|l| l.strip_prefix("Content-Length: "))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

impl Client {
    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.write.write_all(&frame(&msg)).await.unwrap();
        loop {
            let resp = read_msg(&mut self.read).await;
            // Server-initiated requests (e.g. client/registerCapability) block
            // the server until acked.
            if resp.get("method").is_some() {
                if let Some(srv_id) = resp.get("id") {
                    let ack = json!({"jsonrpc": "2.0", "id": srv_id, "result": null});
                    self.write.write_all(&frame(&ack)).await.unwrap();
                }
                continue;
            }
            if resp.get("id").and_then(|v| v.as_i64()) == Some(id) {
                return resp;
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.write.write_all(&frame(&msg)).await.unwrap();
    }

    async fn request_no_params(&mut self, method: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
        self.write.write_all(&frame(&msg)).await.unwrap();
        loop {
            let resp = read_msg(&mut self.read).await;
            if resp.get("method").is_some() {
                if let Some(srv_id) = resp.get("id") {
                    let ack = json!({"jsonrpc": "2.0", "id": srv_id, "result": null});
                    self.write.write_all(&frame(&ack)).await.unwrap();
                }
                continue;
            }
            if resp.get("id").and_then(|v| v.as_i64()) == Some(id) {
                return resp;
            }
        }
    }

    async fn wait_for_diagnostics(&mut self, uri: &str) -> bool {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method").and_then(|v| v.as_str())
                    == Some("textDocument/publishDiagnostics")
                    && msg["params"]["uri"].as_str() == Some(uri)
                {
                    return;
                }
                if let Some(srv_id) = msg.get("id") {
                    let ack = json!({"jsonrpc": "2.0", "id": srv_id, "result": null});
                    self.write.write_all(&frame(&ack)).await.unwrap();
                }
            }
        })
        .await
        .is_ok()
    }

    async fn wait_for_index_ready(&mut self) -> bool {
        tokio::time::timeout(Duration::from_secs(300), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method").and_then(|v| v.as_str()) == Some("$/php-lsp/indexReady") {
                    return;
                }
                if let Some(srv_id) = msg.get("id") {
                    let ack = json!({"jsonrpc": "2.0", "id": srv_id, "result": null});
                    self.write.write_all(&frame(&ack)).await.unwrap();
                }
            }
        })
        .await
        .is_ok()
    }

    async fn wait_for_warm_sweep(&mut self, timeout_secs: u64) -> bool {
        let attempts = timeout_secs * 4;
        for _ in 0..attempts {
            let resp = self.request_no_params("$/php-lsp/debugStats").await;
            if resp["result"]["warm_sweeps_completed"]
                .as_u64()
                .unwrap_or(0)
                >= 1
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        false
    }
}

fn spawn_server() -> Client {
    let (client_stream, server_stream) = tokio::io::duplex(1 << 20);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (service, socket) = LspService::build(Backend::new)
        .custom_method("$/php-lsp/debugStats", Backend::debug_stats)
        .finish();
    tokio::spawn(Server::new(server_read, server_write, socket).serve(service));
    Client {
        write: client_write,
        read: client_read,
        next_id: 1,
    }
}

/// 0-indexed (line, UTF-16 column) of `needle`'s first occurrence in `src`.
/// ASCII-only content, so byte offset == UTF-16 code-unit offset.
fn locate(src: &str, needle: &str) -> (u32, u32) {
    for (line_no, line) in src.lines().enumerate() {
        if let Some(col) = line.find(needle) {
            return (line_no as u32, col as u32);
        }
    }
    panic!("needle {needle:?} not found in source");
}

const TARGETS_SRC: &str = r#"<?php

namespace BenchNs;

const BENCH_GLOBAL_CONST = 1;

function bench_global_function(): int
{
    return 1;
}

class BenchOwner
{
    public const BENCH_CLASS_CONST = 2;

    public int $benchProp = 0;

    public static function benchStaticMethod(): int
    {
        return 1;
    }

    public function benchInstanceMethod(): int
    {
        return 1;
    }

    private function benchPrivateMethod(): int
    {
        return 1;
    }

    public function callPrivate(): int
    {
        return $this->benchPrivateMethod();
    }

    protected function benchProtectedMethod(): int
    {
        return 1;
    }
}

class BenchSub extends BenchOwner
{
    public function useProtected(): int
    {
        return $this->benchProtectedMethod();
    }
}
"#;

const CALLER_SRC: &str = r#"<?php

namespace BenchNs;

function bench_caller(): int
{
    $x = new BenchOwner();
    $x->benchInstanceMethod();
    BenchOwner::benchStaticMethod();
    echo $x->benchProp;
    echo BenchOwner::BENCH_CLASS_CONST;
    echo BENCH_GLOBAL_CONST;
    bench_global_function();
    return 1;
}
"#;

struct Kind {
    label: &'static str,
    needle: &'static str,
}

const KINDS: &[Kind] = &[
    Kind {
        label: "class",
        needle: "BenchOwner",
    },
    Kind {
        label: "function",
        needle: "bench_global_function",
    },
    Kind {
        label: "method (public static)",
        needle: "benchStaticMethod",
    },
    Kind {
        label: "method (public instance)",
        needle: "benchInstanceMethod",
    },
    Kind {
        label: "method (private)",
        needle: "benchPrivateMethod",
    },
    Kind {
        label: "method (protected)",
        needle: "benchProtectedMethod",
    },
    Kind {
        label: "property",
        needle: "benchProp",
    },
    Kind {
        label: "class constant",
        needle: "BENCH_CLASS_CONST",
    },
    Kind {
        label: "global constant",
        needle: "BENCH_GLOBAL_CONST",
    },
];

fn main() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(16)
        .build()
        .unwrap()
        .block_on(run());
}

async fn run() {
    // REFERENCES_BENCH_ROOT overrides the fixture with any real workspace —
    // the two synthetic files below are opened alongside it regardless of
    // that project's own autoload/PSR-4 setup, so the symbol kinds still
    // resolve; only the *candidate scope size* (and therefore the scan cost
    // the fixes below narrow) changes.
    let root = match std::env::var_os("REFERENCES_BENCH_ROOT") {
        Some(p) => PathBuf::from(p),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel"),
    };
    if !root.is_dir() {
        eprintln!(
            "{} missing — run scripts/setup_laravel_fixture.sh, or set REFERENCES_BENCH_ROOT, to enable references_all_kinds",
            root.display()
        );
        return;
    }
    eprintln!("workspace root: {}", root.display());

    let mut c = spawn_server();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": format!("file://{}", root.display()), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    assert!(c.wait_for_index_ready().await, "indexReady timeout");
    assert!(c.wait_for_warm_sweep(600).await, "warm sweep timeout");

    let targets_uri = format!("file://{}/BenchTargets.php", root.display());
    let caller_uri = format!("file://{}/BenchCaller.php", root.display());
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": targets_uri, "languageId": "php", "version": 1, "text": TARGETS_SRC}}),
    )
    .await;
    assert!(
        c.wait_for_diagnostics(&targets_uri).await,
        "targets didOpen diag timeout"
    );
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": caller_uri, "languageId": "php", "version": 1, "text": CALLER_SRC}}),
    )
    .await;
    assert!(
        c.wait_for_diagnostics(&caller_uri).await,
        "caller didOpen diag timeout"
    );

    println!(
        "{:<28} {:>8} {:>10} {:>10} {:>10} {:>10}",
        "kind", "refs", "cold_ms", "warm_p50", "warm_p95", "warm_max"
    );
    for kind in KINDS {
        let (line, col) = locate(TARGETS_SRC, kind.needle);
        let params = json!({
            "textDocument": {"uri": targets_uri},
            "position": {"line": line, "character": col},
            "context": {"includeDeclaration": true},
        });

        let t0 = Instant::now();
        let resp = c.request("textDocument/references", params.clone()).await;
        let cold_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let refs = resp["result"].as_array().map(|a| a.len()).unwrap_or(0);

        let stats_before = c.request_no_params("$/php-lsp/debugStats").await;
        let hits_before = stats_before["result"]["mir_ref_query_cache_hits"]
            .as_u64()
            .unwrap_or(0);
        let subtype_hits_before = stats_before["result"]["mir_subtype_query_cache_hits"]
            .as_u64()
            .unwrap_or(0);
        let scans_before = stats_before["result"]["mir_mention_scans_recorded"]
            .as_u64()
            .unwrap_or(0);

        let mut samples = Vec::with_capacity(10);
        for _ in 0..10 {
            let t = Instant::now();
            c.request("textDocument/references", params.clone()).await;
            samples.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let stats_after = c.request_no_params("$/php-lsp/debugStats").await;
        let hits_after = stats_after["result"]["mir_ref_query_cache_hits"]
            .as_u64()
            .unwrap_or(0);
        let subtype_hits_after = stats_after["result"]["mir_subtype_query_cache_hits"]
            .as_u64()
            .unwrap_or(0);
        let scans_after = stats_after["result"]["mir_mention_scans_recorded"]
            .as_u64()
            .unwrap_or(0);
        println!(
            "{:<28} {:>8} {:>10.3} {:>10.3} {:>10.3} {:>10.3}  ref_hits+{} subtype_hits+{} scans+{}",
            kind.label,
            refs,
            cold_ms,
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100],
            samples.last().unwrap(),
            hits_after - hits_before,
            subtype_hits_after - subtype_hits_before,
            scans_after - scans_before,
        );
    }
}
