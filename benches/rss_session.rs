//! Session-length memory guard (P1b.2): RSS across a sustained edit session.
//!
//! Drives the real LSP server against the pinned Laravel fixture, opens four
//! files, and simulates a long editing session — alternating body edits and
//! declaration edits (the kind that stale sibling analyses) across the open
//! set, with hover/completion/references fired along the way to exercise
//! every request cache. Samples process RSS throughout.
//!
//! The property under guard is P0.2/WS2's promise: memos, interners, and
//! request caches are bounded, so a long session plateaus instead of growing
//! linearly with keystrokes. Gate: RSS growth from the post-index baseline to
//! the end of the session stays under an absolute ceiling.
//!
//! Run with `cargo bench --bench rss_session`. Release mode matters.

use php_lsp::backend::Backend;
use serde_json::{Value, json};

// Match the production binary's allocator — RSS behavior is allocator
// behavior, so measuring with the system allocator would guard the wrong
// thing (src/main.rs sets mimalloc the same way).
#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Heap-profile mode: `cargo bench --bench rss_session --features dhat-heap`
// writes dhat-heap.json attributing retained bytes at exit (use
// RSS_SESSION_ROUNDS to shorten the run — dhat is ~3-5x slower).
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tower_lsp_server::{LspService, Server};

const EDIT_ROUNDS: usize = 75; // 4 edits per round (one per open file) → 300 edits
const RSS_SAMPLE_EVERY: usize = 5; // rounds
/// The session's first ~100 edits warm caches and thread-heap pools; the
/// property under guard is the *tail*: once warm, RSS must plateau. With the
/// production runtime config the tail measures ~40 MB over the last 200
/// edits (decelerating); the uncapped-blocking-pool regression this bench
/// caught grew ~15-20 MB per round here — loudly over either gate.
const TAIL_START_ROUND: usize = 25;
const TAIL_GROWTH_CEILING_MB: f64 = 120.0;
/// Absolute sanity ceiling on end RSS growth from the post-index baseline.
/// The baseline is captured *after* the analysis warm sweep completes (waited
/// via debugStats), so the warm memo table (~80 MB on this fixture) is in the
/// baseline, not counted as session growth; edit-idle re-warms during the run
/// replace memos rather than adding them.
const GROWTH_CEILING_MB: f64 = 400.0;
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
    next_id: u64,
}

impl Client {
    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.write.write_all(&frame(&msg)).await.unwrap();
        let method_owned = method.to_string();
        tokio::time::timeout(Duration::from_secs(60), async {
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
        })
        .await
        .unwrap_or_else(|_| panic!("timed out: {method_owned}"))
    }

    async fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc":"2.0","method":method,"params":params});
        self.write.write_all(&frame(&msg)).await.unwrap();
    }

    async fn wait_for_diagnostics(&mut self, uri: &str) -> bool {
        let uri_val = json!(uri);
        tokio::time::timeout(Duration::from_secs(DIAG_TIMEOUT_SECS), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method") == Some(&json!("textDocument/publishDiagnostics"))
                    && msg["params"]["uri"] == uri_val
                {
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

    /// Like [`Self::request`] but omits `params` — required for handlers
    /// declared without a params argument (`$/php-lsp/debugStats`).
    async fn request_no_params(&mut self, method: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc":"2.0","id":id,"method":method});
        self.write.write_all(&frame(&msg)).await.unwrap();
        let method_owned = method.to_string();
        tokio::time::timeout(Duration::from_secs(60), async {
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
        })
        .await
        .unwrap_or_else(|_| panic!("timed out: {method_owned}"))
    }

    /// Poll `$/php-lsp/debugStats` until the post-index warm sweep completes,
    /// so the RSS baseline deterministically includes the warm memo table
    /// instead of racing it.
    async fn wait_for_warm_sweep(&mut self) -> bool {
        for _ in 0..600 {
            let resp = self.request_no_params("$/php-lsp/debugStats").await;
            if resp["result"]["warm_sweeps_completed"]
                .as_u64()
                .unwrap_or(0)
                >= 1
            {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
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

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    let kb: f64 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0.0);
    kb / 1024.0
}

fn pick_open_files(root: &Path) -> Vec<PathBuf> {
    // Prefer medium-sized real files so the edits exercise real analysis.
    let candidates = [
        "src/Illuminate/Auth/AuthManager.php",
        "src/Illuminate/Cache/CacheManager.php",
        "src/Illuminate/Support/Str.php",
        "src/Illuminate/Collections/Arr.php",
    ];
    candidates
        .iter()
        .map(|p| root.join(p))
        .filter(|p| p.is_file())
        .collect()
}

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    // Mirror the production runtime config (src/main.rs): the blocking-pool
    // cap is load-bearing for session RSS. RSS_SESSION_MAX_BLOCKING overrides
    // for leak-diagnosis runs.
    let cap = std::env::var("RSS_SESSION_MAX_BLOCKING")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(16);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(cap)
        .build()
        .unwrap()
        .block_on(run());
}

async fn run() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel");
    if !root.is_dir() {
        eprintln!(
            "benches/fixtures/laravel missing — skipping (run scripts/setup_laravel_fixture.sh)"
        );
        return;
    }
    let files = pick_open_files(&root);
    assert!(
        files.len() >= 4,
        "expected the pinned fixture layout; found {} of 4 files",
        files.len()
    );

    let mut c = spawn_server();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": format!("file://{}", root.display()), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    assert!(c.wait_for_index_ready().await, "indexReady timeout");
    assert!(c.wait_for_warm_sweep().await, "warm sweep timeout");

    let mut docs: Vec<(String, String)> = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let uri = format!("file://{}", path.display());
        c.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "php", "version": 1, "text": text}}),
        )
        .await;
        assert!(c.wait_for_diagnostics(&uri).await, "didOpen diag timeout");
        docs.push((uri, text));
    }

    let rss_baseline = rss_mb();
    eprintln!("baseline RSS after index + 4 opens: {rss_baseline:.0} MB");

    let rounds: usize = std::env::var("RSS_SESSION_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(EDIT_ROUNDS);
    let mut peak = rss_baseline;
    let mut tail_baseline: Option<f64> = None;
    let mut version = 1i64;
    for round in 0..rounds {
        for (i, (uri, text)) in docs.iter_mut().enumerate() {
            version += 1;
            // Alternate body edits with declaration edits: appending a fresh
            // top-level function each time changes the file's declaration
            // fingerprint, which is what stales sibling analyses and grows
            // memo tables if anything is unbounded. RSS_SESSION_MODE narrows
            // the workload for leak diagnosis: `body` (body-only edits),
            // `samename` (decl edits reusing one name), `norequests` (mixed
            // edits, no hover/completion).
            let mode = std::env::var("RSS_SESSION_MODE").unwrap_or_default();
            if mode == "body" || (round + i) % 2 == 0 {
                text.push(' ');
            } else if mode == "samename" {
                text.push_str(&format!(
                    "\nfunction bench_tmp_{i}(): int {{ return 1; }}\n"
                ));
            } else {
                text.push_str(&format!(
                    "\nfunction bench_tmp_{round}_{i}(): int {{ return {round}; }}\n"
                ));
            }
            c.notify(
                "textDocument/didChange",
                json!({
                    "textDocument": {"uri": uri.as_str(), "version": version},
                    "contentChanges": [{"text": text.as_str()}],
                }),
            )
            .await;
            if !c.wait_for_diagnostics(uri).await {
                eprintln!("edit→diag timeout at round {round} — aborting");
                std::process::exit(1);
            }
            // Exercise the read caches the way a user would.
            if std::env::var("RSS_SESSION_MODE").unwrap_or_default() != "norequests" {
                c.request(
                    "textDocument/hover",
                    json!({"textDocument": {"uri": uri.as_str()}, "position": {"line": 12, "character": 12}}),
                )
                .await;
                c.request(
                    "textDocument/completion",
                    json!({"textDocument": {"uri": uri.as_str()}, "position": {"line": 12, "character": 12}}),
                )
                .await;
            }
        }
        if round == TAIL_START_ROUND {
            tail_baseline = Some(rss_mb());
        }
        if round % RSS_SAMPLE_EVERY == 0 {
            let rss = rss_mb();
            peak = peak.max(rss);
            eprintln!("round {round:>3}/{rounds}: rss {rss:.0} MB (baseline {rss_baseline:.0})");
        }
    }

    let rss_end = rss_mb();
    peak = peak.max(rss_end);
    let growth = rss_end - rss_baseline;
    println!(
        "\nrss_session: baseline {rss_baseline:.0} MB → end {rss_end:.0} MB (peak {peak:.0}), growth {growth:.0} MB over {} edits",
        rounds * 4
    );
    let tail_growth = tail_baseline.map(|t| rss_end - t);
    let tail_ok = match tail_growth {
        Some(t) => {
            println!(
                "tail growth (rounds {TAIL_START_ROUND}+): {t:.0} MB (ceiling {TAIL_GROWTH_CEILING_MB})"
            );
            t <= TAIL_GROWTH_CEILING_MB
        }
        // Short diagnosis runs never reach the tail window — skip that gate.
        None => true,
    };
    let ok = growth <= GROWTH_CEILING_MB && tail_ok;
    println!(
        "gates: {}",
        if ok {
            "OK"
        } else {
            "OVER — session-length memory growth is back!"
        }
    );
    if !ok {
        std::process::exit(1);
    }
}
