//! End-to-end keystroke-latency harness.
//!
//! Drives the real LSP server over in-memory duplex pipes (same path tests
//! take) and measures wall-clock latency for the events users actually feel:
//!
//!   - cold open                   (didOpen → publishDiagnostics)
//!   - cold hover / completion / semantic-tokens
//!   - keystroke → publishDiagnostics  (×N)
//!   - keystroke → hover              (×N)
//!   - keystroke → completion         (×N)
//!
//! Writes `target/perf/edit_latency.json` and `target/perf/edit_latency.md`.
//!
//! Run with `cargo bench --bench edit_latency`. Release mode is important —
//! debug builds skew everything by 10×+.

use php_lsp::backend::Backend;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tower_lsp_server::{LspService, Server};

const EDITS_PER_SCENARIO: usize = 20;
const EDITS_PER_LARGE_SCENARIO: usize = 10;
const WORKSPACE_INDEX_TIMEOUT_SECS: u64 = 90;
const DIAG_TIMEOUT_SECS: u64 = 60;

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

// ---------- minimal client ----------

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
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let resp = read_msg(&mut self.read).await;
                // server→client request: ack null so server isn't blocked
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

    /// Wait for `publishDiagnostics` for `uri`. Returns `None` on timeout
    /// so callers can record-and-continue instead of aborting the whole bench.
    async fn wait_for_diagnostics(&mut self, uri: &str, timeout: Duration) -> Option<Value> {
        let uri_val = json!(uri);
        tokio::time::timeout(timeout, async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method") == Some(&json!("textDocument/publishDiagnostics"))
                    && msg["params"]["uri"] == uri_val
                {
                    return msg;
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
        .ok()
    }

    async fn wait_for_index_ready(&mut self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, async {
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
    }
}

// ---------- protocol helpers ----------

async fn initialize(c: &mut Client, root: Option<&Path>) {
    let root_uri = root.map(|p| format!("file://{}", p.display()));
    let init_params = if let Some(uri) = &root_uri {
        json!({
            "processId": null,
            "rootUri": uri,
            "capabilities": {},
        })
    } else {
        json!({"processId": null, "rootUri": null, "capabilities": {}})
    };
    c.request("initialize", init_params).await;
    c.notify("initialized", json!({})).await;
}

async fn did_open(c: &mut Client, uri: &str, text: &str) {
    c.notify(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": uri,
                "languageId": "php",
                "version": 1,
                "text": text,
            }
        }),
    )
    .await;
}

async fn did_change(c: &mut Client, uri: &str, version: i64, text: &str) {
    c.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": uri, "version": version},
            "contentChanges": [{"text": text}],
        }),
    )
    .await;
}

async fn hover(c: &mut Client, uri: &str, line: u32, character: u32) -> Value {
    c.request(
        "textDocument/hover",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        }),
    )
    .await
}

async fn completion(c: &mut Client, uri: &str, line: u32, character: u32) -> Value {
    c.request(
        "textDocument/completion",
        json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        }),
    )
    .await
}

async fn semantic_tokens_full(c: &mut Client, uri: &str) -> Value {
    c.request(
        "textDocument/semanticTokens/full",
        json!({"textDocument": {"uri": uri}}),
    )
    .await
}

// ---------- percentiles ----------

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    if samples.is_empty() {
        return f64::NAN;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let rank = (p / 100.0) * (samples.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        samples[lo]
    } else {
        let frac = rank - lo as f64;
        samples[lo] * (1.0 - frac) + samples[hi] * frac
    }
}

#[derive(Default, Clone)]
struct Stats {
    n: usize,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
    mean: f64,
}

impl Stats {
    fn from(mut samples: Vec<f64>) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let n = samples.len();
        let sum: f64 = samples.iter().sum();
        let mean = sum / n as f64;
        let p50 = percentile(&mut samples, 50.0);
        let p95 = percentile(&mut samples, 95.0);
        let p99 = percentile(&mut samples, 99.0);
        let max = samples.last().copied().unwrap_or(0.0);
        Self {
            n,
            p50,
            p95,
            p99,
            max,
            mean,
        }
    }
}

// ---------- scenarios ----------

struct ScenarioResult {
    name: String,
    workspace_files: Option<usize>,
    index_ready_ms: Option<f64>,
    cold_open_ms: f64,
    cold_hover_ms: f64,
    cold_completion_ms: f64,
    cold_semantic_tokens_ms: f64,
    edit_to_diag: Stats,
    edit_to_hover: Stats,
    edit_to_completion: Stats,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Run an editing-session scenario.
///
/// - `name` is the label in the report
/// - `workspace_root` (Some) makes the harness wait for `$/php-lsp/indexReady`
/// - `target_path` is the file we open + edit (absolute path)
/// - `cursor_line` / `cursor_char` is where hover/completion fire
async fn run_scenario(
    name: &str,
    workspace_root: Option<&Path>,
    target_path: &Path,
    cursor_line: u32,
    cursor_char: u32,
    workspace_files: Option<usize>,
    edits: usize,
) -> Option<ScenarioResult> {
    eprintln!("\n=== scenario: {name} ===");
    let text = std::fs::read_to_string(target_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", target_path.display()));
    let uri = format!("file://{}", target_path.display());

    let mut c = spawn_server();

    let t0 = Instant::now();
    initialize(&mut c, workspace_root).await;
    let index_ready_ms = if workspace_root.is_some() {
        let ok = c
            .wait_for_index_ready(Duration::from_secs(WORKSPACE_INDEX_TIMEOUT_SECS))
            .await;
        if !ok {
            eprintln!(
                "  skipped: $/php-lsp/indexReady did not arrive within {WORKSPACE_INDEX_TIMEOUT_SECS}s"
            );
            return None;
        }
        Some(ms(t0.elapsed()))
    } else {
        None
    };
    if let Some(v) = index_ready_ms {
        eprintln!("  index_ready: {v:.1} ms");
    }

    // --- cold open ---
    let t = Instant::now();
    did_open(&mut c, &uri, &text).await;
    let diag = c
        .wait_for_diagnostics(&uri, Duration::from_secs(DIAG_TIMEOUT_SECS))
        .await;
    let cold_open_ms = ms(t.elapsed());
    if diag.is_none() {
        eprintln!(
            "  cold_open: TIMEOUT after {cold_open_ms:.0} ms — skipping edit loop for this scenario"
        );
        return Some(ScenarioResult {
            name: name.to_string(),
            workspace_files,
            index_ready_ms,
            cold_open_ms: f64::NAN,
            cold_hover_ms: f64::NAN,
            cold_completion_ms: f64::NAN,
            cold_semantic_tokens_ms: f64::NAN,
            edit_to_diag: Stats::default(),
            edit_to_hover: Stats::default(),
            edit_to_completion: Stats::default(),
        });
    }
    eprintln!("  cold_open: {cold_open_ms:.1} ms");

    // --- cold features ---
    let t = Instant::now();
    hover(&mut c, &uri, cursor_line, cursor_char).await;
    let cold_hover_ms = ms(t.elapsed());

    let t = Instant::now();
    completion(&mut c, &uri, cursor_line, cursor_char).await;
    let cold_completion_ms = ms(t.elapsed());

    let t = Instant::now();
    semantic_tokens_full(&mut c, &uri).await;
    let cold_semantic_tokens_ms = ms(t.elapsed());

    eprintln!(
        "  cold_hover: {cold_hover_ms:.1} ms  cold_completion: {cold_completion_ms:.1} ms  cold_semtok: {cold_semantic_tokens_ms:.1} ms"
    );

    // --- edit loop ---
    let mut diag_samples = Vec::with_capacity(edits);
    let mut hover_samples = Vec::with_capacity(edits);
    let mut comp_samples = Vec::with_capacity(edits);

    let mut edited = text.clone();
    let mut aborted = false;
    for i in 0..edits {
        if aborted {
            break;
        }
        // append a no-op space at end of file each iteration. Cheap, parseable,
        // and forces a re-parse on every keystroke.
        edited.push(' ');
        let mut version = (i * 3 + 2) as i64;

        // (a) edit → publishDiagnostics
        let t = Instant::now();
        did_change(&mut c, &uri, version, &edited).await;
        if c.wait_for_diagnostics(&uri, Duration::from_secs(DIAG_TIMEOUT_SECS))
            .await
            .is_none()
        {
            eprintln!("  edit→diag timeout at iter {i} — aborting loop");
            aborted = true;
            continue;
        }
        diag_samples.push(ms(t.elapsed()));

        // (b) edit → hover  (server has just published diagnostics; this measures
        // hover under "just-edited" cache state).
        edited.push(' ');
        version += 1;
        let t = Instant::now();
        did_change(&mut c, &uri, version, &edited).await;
        hover(&mut c, &uri, cursor_line, cursor_char).await;
        hover_samples.push(ms(t.elapsed()));
        // drain pending diagnostics
        if c.wait_for_diagnostics(&uri, Duration::from_secs(DIAG_TIMEOUT_SECS))
            .await
            .is_none()
        {
            eprintln!("  drain-diag timeout at iter {i} — aborting loop");
            aborted = true;
            continue;
        }

        // (c) edit → completion
        edited.push(' ');
        version += 1;
        let t = Instant::now();
        did_change(&mut c, &uri, version, &edited).await;
        completion(&mut c, &uri, cursor_line, cursor_char).await;
        comp_samples.push(ms(t.elapsed()));
        if c.wait_for_diagnostics(&uri, Duration::from_secs(DIAG_TIMEOUT_SECS))
            .await
            .is_none()
        {
            eprintln!("  drain-diag timeout at iter {i} — aborting loop");
            aborted = true;
        }
    }

    let result = ScenarioResult {
        name: name.to_string(),
        workspace_files,
        index_ready_ms,
        cold_open_ms,
        cold_hover_ms,
        cold_completion_ms,
        cold_semantic_tokens_ms,
        edit_to_diag: Stats::from(diag_samples),
        edit_to_hover: Stats::from(hover_samples),
        edit_to_completion: Stats::from(comp_samples),
    };

    eprintln!(
        "  edit→diag p50/p95/max: {:.1}/{:.1}/{:.1} ms",
        result.edit_to_diag.p50, result.edit_to_diag.p95, result.edit_to_diag.max,
    );
    eprintln!(
        "  edit→hover p50/p95/max: {:.1}/{:.1}/{:.1} ms",
        result.edit_to_hover.p50, result.edit_to_hover.p95, result.edit_to_hover.max,
    );
    eprintln!(
        "  edit→comp  p50/p95/max: {:.1}/{:.1}/{:.1} ms",
        result.edit_to_completion.p50, result.edit_to_completion.p95, result.edit_to_completion.max,
    );

    Some(result)
}

// ---------- output ----------

fn stats_json(s: &Stats) -> Value {
    json!({
        "n": s.n,
        "mean_ms": s.mean,
        "p50_ms": s.p50,
        "p95_ms": s.p95,
        "p99_ms": s.p99,
        "max_ms": s.max,
    })
}

fn write_outputs(results: &[ScenarioResult]) {
    let out_dir = PathBuf::from("target/perf");
    std::fs::create_dir_all(&out_dir).unwrap();

    // JSON
    let json_results: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "name": r.name,
                "workspace_files": r.workspace_files,
                "index_ready_ms": r.index_ready_ms,
                "cold_open_ms": r.cold_open_ms,
                "cold_hover_ms": r.cold_hover_ms,
                "cold_completion_ms": r.cold_completion_ms,
                "cold_semantic_tokens_ms": r.cold_semantic_tokens_ms,
                "edit_to_diag": stats_json(&r.edit_to_diag),
                "edit_to_hover": stats_json(&r.edit_to_hover),
                "edit_to_completion": stats_json(&r.edit_to_completion),
            })
        })
        .collect();
    let json_out = json!({
        "edits_per_scenario": EDITS_PER_SCENARIO,
        "scenarios": json_results,
    });
    std::fs::write(
        out_dir.join("edit_latency.json"),
        serde_json::to_string_pretty(&json_out).unwrap(),
    )
    .unwrap();

    // Markdown
    let mut md = String::new();
    md.push_str("# Edit-latency baseline\n\n");
    md.push_str(&format!(
        "{} edits per scenario. All numbers in milliseconds.\n\n",
        EDITS_PER_SCENARIO
    ));
    md.push_str("## Cold path\n\n");
    md.push_str("| scenario | files | index_ready | cold_open | cold_hover | cold_completion | cold_semtok |\n");
    md.push_str("|---|---:|---:|---:|---:|---:|---:|\n");
    for r in results {
        md.push_str(&format!(
            "| {} | {} | {} | {:.1} | {:.1} | {:.1} | {:.1} |\n",
            r.name,
            r.workspace_files
                .map(|n| n.to_string())
                .unwrap_or_else(|| "—".into()),
            r.index_ready_ms
                .map(|v| format!("{v:.0}"))
                .unwrap_or_else(|| "—".into()),
            r.cold_open_ms,
            r.cold_hover_ms,
            r.cold_completion_ms,
            r.cold_semantic_tokens_ms,
        ));
    }
    md.push_str("\n## Edit → publishDiagnostics\n\n");
    md.push_str("| scenario | p50 | p95 | p99 | max | mean |\n|---|---:|---:|---:|---:|---:|\n");
    for r in results {
        let s = &r.edit_to_diag;
        md.push_str(&format!(
            "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |\n",
            r.name, s.p50, s.p95, s.p99, s.max, s.mean,
        ));
    }
    md.push_str("\n## Edit → hover\n\n");
    md.push_str("| scenario | p50 | p95 | p99 | max | mean |\n|---|---:|---:|---:|---:|---:|\n");
    for r in results {
        let s = &r.edit_to_hover;
        md.push_str(&format!(
            "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |\n",
            r.name, s.p50, s.p95, s.p99, s.max, s.mean,
        ));
    }
    md.push_str("\n## Edit → completion\n\n");
    md.push_str("| scenario | p50 | p95 | p99 | max | mean |\n|---|---:|---:|---:|---:|---:|\n");
    for r in results {
        let s = &r.edit_to_completion;
        md.push_str(&format!(
            "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |\n",
            r.name, s.p50, s.p95, s.p99, s.max, s.mean,
        ));
    }
    std::fs::write(out_dir.join("edit_latency.md"), md).unwrap();

    eprintln!(
        "\nWrote target/perf/edit_latency.json and target/perf/edit_latency.md ({} scenarios)",
        results.len()
    );
}

// ---------- entry point ----------

fn count_php_files(dir: &Path) -> usize {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "php"))
        .count()
}

fn init_tracing() {
    use std::sync::Mutex;
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::fmt::format::FmtSpan;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("off"));
    if filter.to_string() == "off" {
        return;
    }
    let _ = std::fs::create_dir_all("target/perf");
    let file = std::fs::File::create("target/perf/trace.log").expect("create trace.log");
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .with_target(false)
        .with_ansi(false)
        .with_writer(Mutex::new(file))
        .try_init();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    init_tracing();
    let bench_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let benches_fixtures = bench_root.join("benches/fixtures");
    let tests_fixtures = bench_root.join("tests/fixtures");

    let mut results = Vec::new();

    // 1. Single-file scenario: controller.php, no workspace root.
    let controller = benches_fixtures.join("controller.php");
    if let Some(r) = run_scenario(
        "controller-single",
        None,
        &controller,
        // cursor in middle of an identifier; exact landing doesn't matter for latency.
        40, // line
        20, // char
        None,
        EDITS_PER_SCENARIO,
    )
    .await
    {
        results.push(r);
    }

    // 2. Laravel fixture: ~2.9k files, single open file.
    let laravel_root = benches_fixtures.join("laravel");
    if laravel_root.is_dir() {
        let files = count_php_files(&laravel_root);
        // pick a controller-style file to open
        let candidates = [
            "src/Http/Controllers/Controller.php",
            "app/Http/Controllers/Controller.php",
            "src/Foundation/Application.php",
        ];
        let target = candidates
            .iter()
            .map(|p| laravel_root.join(p))
            .find(|p| p.is_file())
            .or_else(|| {
                walkdir::WalkDir::new(&laravel_root)
                    .max_depth(6)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .find(|e| e.path().extension().is_some_and(|x| x == "php"))
                    .map(|e| e.path().to_path_buf())
            });
        if let Some(target) = target {
            if let Some(r) = run_scenario(
                "laravel-warm",
                Some(&laravel_root),
                &target,
                10,
                10,
                Some(files),
                EDITS_PER_LARGE_SCENARIO,
            )
            .await
            {
                results.push(r);
            }
        } else {
            eprintln!("laravel fixture has no .php files — skipping");
        }
    } else {
        eprintln!(
            "benches/fixtures/laravel missing — skipping (run scripts/setup_laravel_fixture.sh to enable)"
        );
    }

    // 3. Symfony demo: ~5.2k files.
    let symfony_root = tests_fixtures.join("symfony-demo");
    if symfony_root.is_dir() {
        let files = count_php_files(&symfony_root);
        let target = walkdir::WalkDir::new(symfony_root.join("src"))
            .max_depth(6)
            .into_iter()
            .filter_map(|e| e.ok())
            .find(|e| e.path().extension().is_some_and(|x| x == "php"))
            .map(|e| e.path().to_path_buf());
        if let Some(target) = target
            && let Some(r) = run_scenario(
                "symfony-warm",
                Some(&symfony_root),
                &target,
                10,
                10,
                Some(files),
                EDITS_PER_LARGE_SCENARIO,
            )
            .await
        {
            results.push(r);
        }
    } else {
        eprintln!("tests/fixtures/symfony-demo missing — skipping");
    }

    write_outputs(&results);
}
