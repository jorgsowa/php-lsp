//! Cold/warm start wall-time harness (P1b.2).
//!
//! Measures what a user feels when opening a workspace: how long until the
//! server gives the first *correct* answer, not how long until it claims
//! readiness. Drives the real LSP server over in-memory duplex pipes against
//! the pinned Laravel fixture and probes two cross-file features from t0:
//!
//!   - goto-definition on `Str` (must land in src/Illuminate/Support/Str.php)
//!   - completion after `Str::` (must offer `camel`)
//!
//! polled every 25 ms until each first succeeds, alongside the time to
//! `$/php-lsp/indexReady` and the process RSS once ready.
//!
//! Each scenario runs in its own process (the bench re-execs itself) so RSS
//! is honest and `XDG_CACHE_HOME` isolates the disk cache: `cold` starts from
//! an empty cache dir, `warm` reuses the one `cold` populated.
//!
//! Writes `target/perf/start_time.json` and `.md`.
//! Run with `cargo bench --bench start_time`. Release mode matters.

use php_lsp::backend::Backend;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tower_lsp_server::{LspService, Server};

const INDEX_READY_TIMEOUT_SECS: u64 = 180;
const PROBE_INTERVAL_MS: u64 = 25;

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

// ---------- client (tracks $/php-lsp/indexReady while reading) ----------

struct Client {
    write: WriteHalf<DuplexStream>,
    read: ReadHalf<DuplexStream>,
    next_id: u64,
    index_ready_at: Option<Instant>,
}

impl Client {
    /// Handle a server→client message that is not the awaited response:
    /// record indexReady, ack server requests.
    async fn absorb(&mut self, msg: &Value) {
        if msg.get("method") == Some(&json!("$/php-lsp/indexReady"))
            && self.index_ready_at.is_none()
        {
            self.index_ready_at = Some(Instant::now());
        }
        if msg.get("method").is_some()
            && let Some(id) = msg.get("id")
        {
            let ack = json!({"jsonrpc":"2.0","id":id,"result":null});
            self.write.write_all(&frame(&ack)).await.unwrap();
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        self.write.write_all(&frame(&msg)).await.unwrap();
        let method_owned = method.to_string();
        loop {
            let resp = tokio::time::timeout(Duration::from_secs(60), read_msg(&mut self.read))
                .await
                .unwrap_or_else(|_| panic!("timed out awaiting response: {method_owned}"));
            if resp.get("method").is_some() {
                self.absorb(&resp).await;
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

    async fn wait_for_index_ready(&mut self, timeout: Duration) -> bool {
        if self.index_ready_at.is_some() {
            return true;
        }
        tokio::time::timeout(timeout, async {
            loop {
                let msg = read_msg(&mut self.read).await;
                self.absorb(&msg).await;
                if self.index_ready_at.is_some() {
                    return;
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
        next_id: 1,
        index_ready_at: None,
    }
}

// ---------- probe file ----------

const PROBE_TEXT: &str = "<?php\n\nnamespace Bench;\n\nuse Illuminate\\Support\\Str;\n\nclass StartProbe\n{\n    public function run(): string\n    {\n        return Str::camel('foo_bar');\n    }\n}\n";
// Line 10: `        return Str::camel('foo_bar');`
const DEF_LINE: u32 = 10;
const DEF_CHAR: u32 = 16; // inside `Str`
const COMPLETION_LINE: u32 = 10;
const COMPLETION_CHAR: u32 = 20; // right after `Str::`

fn definition_is_correct(resp: &Value) -> bool {
    let result = &resp["result"];
    let uris: Vec<&str> = match result {
        Value::Array(locs) => locs.iter().filter_map(|l| l["uri"].as_str()).collect(),
        Value::Object(_) => result["uri"].as_str().into_iter().collect(),
        _ => return false,
    };
    uris.iter()
        .any(|u| u.ends_with("Illuminate/Support/Str.php"))
}

fn completion_is_correct(resp: &Value) -> bool {
    let items = match &resp["result"] {
        Value::Array(items) => items,
        Value::Object(o) => match o.get("items") {
            Some(Value::Array(items)) => items,
            _ => return false,
        },
        _ => return false,
    };
    items.iter().any(|i| i["label"].as_str() == Some("camel"))
}

// ---------- scenario runner (child process) ----------

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

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

async fn run_scenario(name: &str, root: &Path) -> Value {
    let root_uri = format!("file://{}", root.display());
    let probe_uri = format!("{root_uri}/bench_start_probe.php");

    let mut c = spawn_server();

    let t0 = Instant::now();
    let t = Instant::now();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": root_uri, "capabilities": {}}),
    )
    .await;
    let initialize_ms = ms(t.elapsed());
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": probe_uri, "languageId": "php", "version": 1, "text": PROBE_TEXT}}),
    )
    .await;

    // Poll both probes until each first answers correctly.
    let mut first_def_ms: Option<f64> = None;
    let mut first_completion_ms: Option<f64> = None;
    let mut attempt = 0usize;
    let deadline = t0 + Duration::from_secs(INDEX_READY_TIMEOUT_SECS);
    while (first_def_ms.is_none() || first_completion_ms.is_none()) && Instant::now() < deadline {
        attempt += 1;
        if first_def_ms.is_none() {
            let ta = Instant::now();
            let resp = c
                .request(
                    "textDocument/definition",
                    json!({"textDocument": {"uri": probe_uri},
                           "position": {"line": DEF_LINE, "character": DEF_CHAR}}),
                )
                .await;
            if std::env::var_os("START_TIME_TRACE").is_some() && attempt <= 8 {
                eprintln!(
                    "  [{name}] def attempt {attempt}: {:.0} ms rtt, result={}",
                    ms(ta.elapsed()),
                    resp["result"]
                );
            }
            if definition_is_correct(&resp) {
                first_def_ms = Some(ms(t0.elapsed()));
                eprintln!(
                    "  [{name}] first correct definition: {:.0} ms",
                    first_def_ms.unwrap()
                );
            }
        }
        if first_completion_ms.is_none() {
            let resp = c
                .request(
                    "textDocument/completion",
                    json!({"textDocument": {"uri": probe_uri},
                           "position": {"line": COMPLETION_LINE, "character": COMPLETION_CHAR}}),
                )
                .await;
            if completion_is_correct(&resp) {
                first_completion_ms = Some(ms(t0.elapsed()));
                eprintln!(
                    "  [{name}] first correct completion: {:.0} ms",
                    first_completion_ms.unwrap()
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(PROBE_INTERVAL_MS)).await;
    }

    let ready = c
        .wait_for_index_ready(deadline.saturating_duration_since(Instant::now()))
        .await;
    let index_ready_ms = if ready {
        Some(ms(c.index_ready_at.unwrap() - t0))
    } else {
        None
    };
    eprintln!(
        "  [{name}] indexReady: {} ms",
        index_ready_ms.map_or("TIMEOUT".into(), |v| format!("{v:.0}"))
    );

    // One more paint of each feature post-ready, then measure the footprint.
    let _ = c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": probe_uri},
                   "position": {"line": COMPLETION_LINE, "character": COMPLETION_CHAR}}),
        )
        .await;
    let rss = rss_mb();
    eprintln!("  [{name}] rss: {rss:.0} MB");

    json!({
        "scenario": name,
        "initialize_ms": initialize_ms,
        "first_definition_ms": first_def_ms,
        "first_completion_ms": first_completion_ms,
        "index_ready_ms": index_ready_ms,
        "rss_mb": rss,
        "definition_before_ready": both_before(first_def_ms, index_ready_ms),
        "completion_before_ready": both_before(first_completion_ms, index_ready_ms),
    })
}

fn both_before(probe: Option<f64>, ready: Option<f64>) -> bool {
    matches!((probe, ready), (Some(p), Some(r)) if p < r)
}

// ---------- orchestrator ----------

fn fixture_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel");
    root.is_dir().then_some(root)
}

fn run_child(scenario: &str, root: &Path, cache_dir: &Path) -> Option<Value> {
    let exe = std::env::current_exe().expect("current_exe");
    let out = std::process::Command::new(exe)
        .args(["--scenario", scenario, "--root", root.to_str().unwrap()])
        .env("XDG_CACHE_HOME", cache_dir)
        .stderr(std::process::Stdio::inherit())
        .output()
        .expect("spawn scenario child");
    if !out.status.success() {
        eprintln!("scenario {scenario} child failed: {}", out.status);
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().rev().find(|l| l.starts_with("RESULT "))?;
    serde_json::from_str(line.trim_start_matches("RESULT ")).ok()
}

fn fmt_opt(v: Option<f64>) -> String {
    v.map_or("—".into(), |v| format!("{v:.0}"))
}

fn main() {
    // Child mode: run one scenario in-process and emit RESULT json.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--scenario") {
        let scenario = args[i + 1].clone();
        let root =
            PathBuf::from(args[args.iter().position(|a| a == "--root").unwrap() + 1].clone());
        // Mirror the production runtime config (src/main.rs) — the blocking
        // pool cap shapes both scan throughput and RSS.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .max_blocking_threads(16)
            .build()
            .unwrap();
        let result = rt.block_on(run_scenario(&scenario, &root));
        println!("RESULT {result}");
        return;
    }

    let Some(root) = fixture_root() else {
        eprintln!(
            "benches/fixtures/laravel missing — skipping (run scripts/setup_laravel_fixture.sh)"
        );
        return;
    };

    let perf_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/perf");
    std::fs::create_dir_all(&perf_dir).unwrap();
    let cache_dir = perf_dir.join("start_time_cache");
    let _ = std::fs::remove_dir_all(&cache_dir);
    std::fs::create_dir_all(&cache_dir).unwrap();

    eprintln!("=== scenario: cold (empty disk cache) ===");
    let cold = run_child("cold", &root, &cache_dir);
    eprintln!("=== scenario: warm (disk cache from cold run) ===");
    let warm = run_child("warm", &root, &cache_dir);

    let results: Vec<Value> = [cold, warm].into_iter().flatten().collect();
    if results.is_empty() {
        eprintln!("no scenario produced a result");
        std::process::exit(1);
    }

    let mut md = String::from(
        "# start_time — cold/warm start wall time (Laravel fixture)\n\n\
         | scenario | initialize ms | first goto-def ms | first completion ms | indexReady ms | RSS MB |\n\
         |---|---:|---:|---:|---:|---:|\n",
    );
    println!(
        "\n{:>8}  {:>13}  {:>17}  {:>19}  {:>13}  {:>7}",
        "scenario",
        "initialize ms",
        "first goto-def ms",
        "first completion ms",
        "indexReady ms",
        "RSS MB"
    );
    for r in &results {
        let row = (
            r["scenario"].as_str().unwrap_or("?"),
            r["initialize_ms"].as_f64().unwrap_or(f64::NAN),
            r["first_definition_ms"].as_f64(),
            r["first_completion_ms"].as_f64(),
            r["index_ready_ms"].as_f64(),
            r["rss_mb"].as_f64().unwrap_or(f64::NAN),
        );
        println!(
            "{:>8}  {:>13.0}  {:>17}  {:>19}  {:>13}  {:>7.0}",
            row.0,
            row.1,
            fmt_opt(row.2),
            fmt_opt(row.3),
            fmt_opt(row.4),
            row.5
        );
        md.push_str(&format!(
            "| {} | {:.0} | {} | {} | {} | {:.0} |\n",
            row.0,
            row.1,
            fmt_opt(row.2),
            fmt_opt(row.3),
            fmt_opt(row.4),
            row.5
        ));
    }

    std::fs::write(
        perf_dir.join("start_time.json"),
        serde_json::to_string_pretty(&json!({"results": results})).unwrap(),
    )
    .unwrap();
    std::fs::write(perf_dir.join("start_time.md"), md).unwrap();
    eprintln!("\nwrote target/perf/start_time.{{json,md}}");
}
