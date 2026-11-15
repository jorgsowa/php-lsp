//! Live heap after a cold boot versus a restart from its cache (issue #269).
//!
//! Both end up holding the same reference postings; the cold boot's warm sweep
//! must not also keep every swept file's analysis alive. Each boot runs in a
//! fresh child process (so interners and allocator state start empty) and
//! reports live heap bytes once its warm sweep completes.

use std::alloc::{GlobalAlloc, Layout};
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tower_lsp_server::Server;
use tower_lsp_server::ls_types::Uri;

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { mimalloc::MiMalloc.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { mimalloc::MiMalloc.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { mimalloc::MiMalloc.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const FILES: usize = 400;
/// How much more live heap a cold boot may hold than a cached restart, in percent.
const COLD_OVERHEAD_CEILING_PCT: isize = 30;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--child") {
        let (workspace, cache) = (Path::new(&args[pos + 1]), Path::new(&args[pos + 2]));
        let live = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(child(workspace, cache));
        println!("LIVE {live}");
        std::process::exit(0);
    }
    // libtest flags such as `--list` or a name filter: nothing else to report.
    if args.iter().any(|a| a == "--list") {
        println!("warm_sweep_memory: test");
        return;
    }

    let workspace = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    write_workspace(workspace.path());
    let cold = run_child("cold", workspace.path(), cache.path());
    let cached = run_child("cached", workspace.path(), cache.path());
    let overhead_pct = (cold - cached) * 100 / cached;
    println!(
        "warm_sweep_memory: {FILES} files, live heap {} KB after a cold boot, {} KB after a cached restart (+{overhead_pct}%, ceiling {COLD_OVERHEAD_CEILING_PCT}%)",
        cold / 1024,
        cached / 1024
    );
    assert!(
        overhead_pct <= COLD_OVERHEAD_CEILING_PCT,
        "a cold boot keeps {overhead_pct}% more live heap than a cached restart"
    );
}

fn run_child(mode: &str, workspace: &Path, cache: &Path) -> isize {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--child")
        .args([workspace, cache])
        .output()
        .expect("spawn child");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{mode} child failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
        .lines()
        .find_map(|l| l.strip_prefix("LIVE "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or_else(|| panic!("{mode} child printed no LIVE line:\n{stdout}"))
}

/// A class per file whose methods call into neighbouring classes, so analysis
/// output is large next to the declarations.
fn write_workspace(root: &Path) {
    for i in 0..FILES {
        let next = (i + 1) % FILES;
        let prev = (i + FILES - 1) % FILES;
        let mut body = String::new();
        for m in 0..6 {
            body.push_str(&format!(
                "    public function m{m}(int $n, ?C{next} $other = null): int {{
        $total = 0;
        foreach (range(0, $n) as $k) {{
            if ($other !== null && $k % 2 === 0) {{
                $total += $other->m{m}($k - 1);
            }} elseif ($k > {m}) {{
                $total += (new C{prev}())->value() + strlen((string) $k);
            }} else {{
                $total -= $this->value();
            }}
        }}
        $list = array_map(fn (int $v): int => $v * {m}, [$total, $n]);
        return $total + count($list) + $this->value();
    }}
"
            ));
        }
        let src = format!(
            "<?php\nnamespace App;\n\nclass C{i}\n{{\n    private int $value = {i};\n\n    public function value(): int\n    {{\n        return $this->value;\n    }}\n\n{body}}}\n"
        );
        std::fs::write(root.join(format!("C{i}.php")), src).unwrap();
    }
}

async fn child(workspace: &Path, cache: &Path) -> isize {
    let (client_stream, server_stream) = tokio::io::duplex(1 << 20);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let (read, write) = tokio::io::split(client_stream);
    let (service, socket) = php_lsp::backend::build_lsp_service();
    tokio::spawn(Server::new(server_read, server_write, socket).serve(service));
    let mut c = Client {
        read,
        write,
        next_id: 1,
        index_ready: false,
    };
    c.request(
        "initialize",
        Some(json!({
            "processId": null,
            "rootUri": Uri::from_file_path(workspace).unwrap().as_str(),
            "capabilities": {},
            "initializationOptions": {
                "cachePath": cache,
                "diagnostics": {"enabled": false},
            },
        })),
    )
    .await;
    c.notify("initialized", json!({})).await;

    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let stats = c.request("$/php-lsp/debugStats", None).await["result"].clone();
        if c.index_ready && stats["warm_sweeps_completed"].as_u64().unwrap_or(0) >= 1 {
            assert_eq!(
                stats["workspace_file_count"].as_u64(),
                Some(FILES as u64),
                "indexed file count"
            );
            break;
        }
        assert!(Instant::now() < deadline, "server never settled");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Let background tasks spawned around indexReady wind down.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let live = LIVE.load(Ordering::Relaxed);
    c.request("shutdown", None).await;
    live
}

struct Client {
    read: ReadHalf<DuplexStream>,
    write: WriteHalf<DuplexStream>,
    next_id: i64,
    index_ready: bool,
}

impl Client {
    async fn send(&mut self, msg: Value) {
        let body = serde_json::to_string(&msg).unwrap();
        let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        self.write.write_all(framed.as_bytes()).await.unwrap();
    }

    async fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await;
    }

    async fn request(&mut self, method: &str, params: Option<Value>) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if let Some(p) = params {
            msg["params"] = p;
        }
        self.send(msg).await;
        loop {
            let msg = self.read_msg().await;
            if let Some(m) = msg.get("method") {
                if m == "$/php-lsp/indexReady" {
                    self.index_ready = true;
                }
                if let Some(server_id) = msg.get("id") {
                    self.send(json!({"jsonrpc": "2.0", "id": server_id, "result": null}))
                        .await;
                }
                continue;
            }
            if msg["id"].as_i64() == Some(id) {
                return msg;
            }
        }
    }

    async fn read_msg(&mut self) -> Value {
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            header.push(self.read.read_u8().await.unwrap());
        }
        let len: usize = String::from_utf8_lossy(&header)
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let mut body = vec![0u8; len];
        self.read.read_exact(&mut body).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }
}
