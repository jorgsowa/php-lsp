/// Scratch diagnostic (not a permanent bench): fires a batch of requests per
/// LSP feature (hover, completion, references, code_lens, semantic tokens)
/// against the Laravel fixture and reports RSS deltas + a dhat heap profile,
/// so allocation can be attributed to a specific feature's handler stack.
///
/// cargo run --release --features dhat-heap --bin mem_lsp_features
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;
#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::path::{Path, PathBuf};
use std::time::Duration;

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
            // the server until acked — without this, initialization deadlocks
            // before the workspace scan ever starts.
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
                if msg.get("method").and_then(|v| v.as_str()) == Some("textDocument/publishDiagnostics")
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
            if resp["result"]["warm_sweeps_completed"].as_u64().unwrap_or(0) >= 1 {
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

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    let kb: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0.0);
    kb / 1024.0
}

fn open_files(root: &Path) -> Vec<PathBuf> {
    [
        "src/Illuminate/Auth/AuthManager.php",
        "src/Illuminate/Cache/CacheManager.php",
        "src/Illuminate/Support/Str.php",
    ]
    .iter()
    .map(|p| root.join(p))
    .filter(|p| p.is_file())
    .collect()
}

const ITERS: usize = 30;

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

async fn run() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel");
    if !root.is_dir() {
        eprintln!("benches/fixtures/laravel missing — run scripts/setup_laravel_fixture.sh");
        return;
    }
    let files = open_files(&root);
    assert!(!files.is_empty(), "expected at least one fixture file");

    let mut c = spawn_server();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": format!("file://{}", root.display()), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    assert!(c.wait_for_index_ready().await, "indexReady timeout");
    assert!(c.wait_for_warm_sweep(600).await, "warm sweep timeout");

    let mut uris = Vec::new();
    for path in &files {
        let text = std::fs::read_to_string(path).unwrap();
        let uri = format!("file://{}", path.display());
        c.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "php", "version": 1, "text": text}}),
        )
        .await;
        assert!(c.wait_for_diagnostics(&uri).await, "didOpen diag timeout");
        uris.push(uri);
    }

    let baseline = rss_mb();
    println!("baseline RSS after index + {} opens: {baseline:.1} MB", uris.len());

    macro_rules! feature_block {
        ($name:expr, $method:expr, $params:expr) => {{
            let before = rss_mb();
            for _ in 0..ITERS {
                for uri in &uris {
                    c.request($method, $params(uri)).await;
                }
            }
            let after = rss_mb();
            println!(
                "{:<12} {} reqs   rss {:.1} -> {:.1} MB (Δ {:+.1}, cum Δ {:+.1})",
                $name,
                ITERS * uris.len(),
                before,
                after,
                after - before,
                after - baseline
            );
            // Warm-latency check: 10 more identical requests per file, timed
            // individually now that the caches are fully warm.
            let mut samples = Vec::with_capacity(10 * uris.len());
            for _ in 0..10 {
                for uri in &uris {
                    let t = std::time::Instant::now();
                    c.request($method, $params(uri)).await;
                    samples.push(t.elapsed().as_secs_f64() * 1000.0);
                }
            }
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
            println!(
                "  {} warm latency: p50 {:.3} ms, p95 {:.3} ms, max {:.3} ms",
                $name,
                samples[samples.len() / 2],
                samples[samples.len() * 95 / 100],
                samples.last().unwrap(),
            );
        }};
    }

    feature_block!("hover", "textDocument/hover", |uri: &String| json!({
        "textDocument": {"uri": uri}, "position": {"line": 20, "character": 12}
    }));

    feature_block!("completion", "textDocument/completion", |uri: &String| json!({
        "textDocument": {"uri": uri}, "position": {"line": 20, "character": 12}
    }));

    feature_block!("references", "textDocument/references", |uri: &String| json!({
        "textDocument": {"uri": uri}, "position": {"line": 20, "character": 12},
        "context": {"includeDeclaration": true}
    }));

    feature_block!("code_lens", "textDocument/codeLens", |uri: &String| json!({
        "textDocument": {"uri": uri}
    }));

    feature_block!("semantic_tokens", "textDocument/semanticTokens/full", |uri: &String| json!({
        "textDocument": {"uri": uri}
    }));

    let end = rss_mb();
    println!("\nfinal: baseline {baseline:.1} MB -> end {end:.1} MB (total Δ {:+.1})", end - baseline);
    let stats = c.request_no_params("$/php-lsp/debugStats").await;
    println!(
        "cache sizes: workspace_file_count={} mir_mention_scans_recorded={} text_cache_len={} decl_fingerprints_len={} analysis_cache_len={} parsed_cache_len={}",
        stats["result"]["workspace_file_count"],
        stats["result"]["mir_mention_scans_recorded"],
        stats["result"]["text_cache_len"],
        stats["result"]["decl_fingerprints_len"],
        stats["result"]["analysis_cache_len"],
        stats["result"]["parsed_cache_len"],
    );
}
