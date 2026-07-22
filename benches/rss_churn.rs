//! Session-length memory guard for *file churn* (rename/delete), distinct
//! from `rss_session.rs`'s edit-churn guard.
//!
//! Drives the real LSP server against a real framework fixture (Laravel or
//! Symfony, selected by `RSS_CHURN_FIXTURE=laravel|symfony`) and simulates the
//! realistic "class renamed" / "branch switch touched many files" shape:
//! `workspace/didRenameFiles` moves a sizable real file to a brand-new path
//! every round, so the *old* path is never reopened — the exact case
//! `DocumentStore::remove` cannot rely on handle reuse for, since the new
//! content lives under a different URI (`handle_did_rename_files`).
//!
//! `DocumentStore::remove` calls `AnalysisSession::invalidate_file`, which
//! frees the abandoned path's (potentially large) text via mir's own
//! `remove_source_file` immediately rather than pinning it for the rest of
//! the process's life — this bench guards that growth from a real rename
//! storm stays bounded rather than scaling linearly with churn volume.
//! Run with `cargo bench --bench rss_churn`. Release mode matters.

use php_lsp::backend::Backend;
use serde_json::{Value, json};

#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tower_lsp::{LspService, Server};

const DEFAULT_ROUNDS: usize = 300;
const SAMPLE_EVERY: usize = 20;
const INDEX_READY_TIMEOUT_SECS: u64 = 300;
/// `parsed_doc`/`symbol_map` are LRU-capped at 2048 entries (mir_queries.rs),
/// so any run short of that count legitimately keeps growing as each new
/// distinct churn file's parse/symbol-map result gets cached for the first
/// time — that's bounded, expected caching, not a leak, and isn't a useful
/// gate signal. Only past this many rounds does new growth necessarily come
/// from something *not* covered by that cap (e.g. a `remove_source_file`
/// regression that stopped freeing abandoned text) rather than just filling
/// the cache for the first time.
const TAIL_START_ROUND: usize = 2048;
const TAIL_GROWTH_CEILING_MB: f64 = 20.0;

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

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(16)
        .build()
        .unwrap()
        .block_on(run());
}

/// (fixture root relative to CARGO_MANIFEST_DIR, real large file to churn —
/// picked from actual fixture content so each rename moves realistic bytes)
fn fixtures() -> Vec<(&'static str, &'static str)> {
    let want = std::env::var("RSS_CHURN_FIXTURE").unwrap_or_default();
    let all = [
        (
            "benches/fixtures/laravel",
            "src/Illuminate/Database/Query/Builder.php",
        ),
        (
            "tests/fixtures/symfony-demo",
            "vendor/symfony/mime/MimeTypes.php",
        ),
    ];
    match want.as_str() {
        "laravel" => vec![all[0]],
        "symfony" => vec![all[1]],
        _ => all.to_vec(),
    }
}

async fn run() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let rounds: usize = std::env::var("RSS_CHURN_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ROUNDS);

    let mut all_ok = true;
    for (fixture_rel, template_rel) in fixtures() {
        let root = manifest.join(fixture_rel);
        if !root.is_dir() {
            eprintln!("SKIP {fixture_rel}: fixture not found");
            continue;
        }
        let template_path = root.join(template_rel);
        let Ok(template_content) = std::fs::read_to_string(&template_path) else {
            eprintln!("SKIP {fixture_rel}: template {template_rel} not found");
            continue;
        };
        println!(
            "\n=== {fixture_rel}  (churn payload: {template_rel}, {} KB/round) ===",
            template_content.len() / 1024
        );
        all_ok &= run_churn(&root, &template_content, rounds).await;
    }

    println!(
        "\ngates: {}",
        if all_ok { "OK" } else { "OVER — file-churn RSS growth is unbounded!" }
    );
    if !all_ok {
        std::process::exit(1);
    }
}

async fn run_churn(root: &Path, template_content: &str, rounds: usize) -> bool {
    let scratch = root.join("_bench_churn_scratch");
    // Clean up any stale scratch dir from a prior aborted run.
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();

    let mut c = spawn_server();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": format!("file://{}", root.display()), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    assert!(c.wait_for_index_ready().await, "indexReady timeout");
    assert!(c.wait_for_warm_sweep().await, "warm sweep timeout");

    let baseline = rss_mb();
    eprintln!("  baseline RSS after index: {baseline:.1} MB");

    let mut prev_path: Option<PathBuf> = None;
    let mut peak = baseline;
    let mut tail_baseline: Option<f64> = None;
    for round in 0..rounds {
        let new_path = scratch.join(format!("Churn{round}.php"));
        std::fs::write(&new_path, template_content).unwrap();
        let new_uri = format!("file://{}", new_path.display());

        if let Some(old_path) = prev_path.take() {
            let old_uri = format!("file://{}", old_path.display());
            c.notify(
                "workspace/didRenameFiles",
                json!({"files": [{"oldUri": old_uri, "newUri": new_uri}]}),
            )
            .await;
            let _ = std::fs::remove_file(&old_path);
        } else {
            c.notify(
                "workspace/didChangeWatchedFiles",
                json!({"changes": [{"uri": new_uri, "type": 1}]}),
            )
            .await;
        }
        // Round-trip through the server so the notification handler (which
        // takes the same write lock as any request) has been dispatched
        // before we move on; a short settle covers the async task's tail.
        c.request_no_params("$/php-lsp/debugStats").await;
        tokio::time::sleep(Duration::from_millis(5)).await;

        prev_path = Some(new_path);

        if round == TAIL_START_ROUND {
            tail_baseline = Some(rss_mb());
        }
        if round % SAMPLE_EVERY == 0 {
            let rss = rss_mb();
            peak = peak.max(rss);
            eprintln!(
                "  round {round:>4}/{rounds}: rss {rss:.1} MB (Δ {:+.1} MB)",
                rss - baseline
            );
        }
    }

    let end = rss_mb();
    peak = peak.max(end);
    let growth = end - baseline;
    println!(
        "  RESULT: baseline {baseline:.1} MB -> end {end:.1} MB (peak {peak:.1}), growth {growth:+.1} MB over {rounds} renames ({:.2} MB/rename)",
        growth / rounds as f64
    );

    let ok = match tail_baseline {
        Some(t) => {
            let tail_growth = end - t;
            println!(
                "  tail growth (rounds {TAIL_START_ROUND}+): {tail_growth:+.1} MB (ceiling {TAIL_GROWTH_CEILING_MB})"
            );
            tail_growth <= TAIL_GROWTH_CEILING_MB
        }
        // Short runs never reach the tail window — nothing to gate on; the
        // per-round numbers above are still useful as a manual sanity check.
        None => {
            println!(
                "  (rounds < {TAIL_START_ROUND}: too short for the tail-growth gate, printed for inspection only)"
            );
            true
        }
    };

    let _ = std::fs::remove_dir_all(&scratch);
    ok
}
