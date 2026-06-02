//! Indexing performance benchmarks against real-world PHP codebases.
//!
//! Run with:
//!   cargo test --test workspace indexing_perf -- --ignored --nocapture
//!
//! Prepare corpora once:
//!   curl -L https://wordpress.org/latest.zip | unzip - -d /tmp/
//!   curl -L https://github.com/laravel/framework/archive/refs/heads/11.x.zip | unzip - -d /tmp/
//!   mv /tmp/framework-11.x /tmp/laravel-framework

use super::*;

const WP_PATH: &str = "/tmp/wordpress";
const LF_PATH: &str = "/tmp/laravel-framework";

fn cache_root() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .map(|d| d.join("php-lsp"))
}

fn wipe_cache() {
    if let Some(d) = cache_root() {
        if d.exists() {
            let _ = std::fs::remove_dir_all(&d);
        }
    }
}

async fn measure(label: &str, path: &str, wipe: bool, timeout_secs: u64) -> std::time::Duration {
    if wipe {
        wipe_cache();
    }
    let start = std::time::Instant::now();
    let mut s = TestServer::with_root(path).await;
    s.wait_for_index_ready_secs(timeout_secs).await;
    let elapsed = start.elapsed();
    let n = std::fs::read_dir(path)
        .map(|_| walkdir_count(path))
        .unwrap_or(0);
    println!("{label:<12}  {elapsed:.2?}   {n} PHP files   {path}");
    elapsed
}

fn walkdir_count(root: &str) -> usize {
    fn walk(dir: &std::path::Path, count: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') {
                    walk(&path, count);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("php") {
                *count += 1;
            }
        }
    }
    let mut n = 0;
    walk(std::path::Path::new(root), &mut n);
    n
}

// ── WordPress ────────────────────────────────────────────────────────────────

#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn indexing_perf_cold_start() {
    if !std::path::Path::new(WP_PATH).is_dir() {
        println!("SKIP: {WP_PATH} not found");
        return;
    }
    let elapsed = measure("WP COLD", WP_PATH, true, 30).await;
    assert!(elapsed.as_secs() < 30, "cold-start took {elapsed:.2?}");
}

#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn indexing_perf_warm_start() {
    if !std::path::Path::new(WP_PATH).is_dir() {
        println!("SKIP: {WP_PATH} not found");
        return;
    }
    measure("WP COLD", WP_PATH, true, 30).await; // populate cache
    let elapsed = measure("WP WARM", WP_PATH, false, 5).await;
    assert!(elapsed.as_secs() < 5, "warm-start took {elapsed:.2?}");
}

// ── Laravel framework ────────────────────────────────────────────────────────

#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn indexing_perf_laravel_cold_start() {
    if !std::path::Path::new(LF_PATH).is_dir() {
        println!("SKIP: {LF_PATH} not found");
        return;
    }
    let elapsed = measure("LF COLD", LF_PATH, true, 30).await;
    assert!(elapsed.as_secs() < 30, "cold-start took {elapsed:.2?}");
}

#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn indexing_perf_laravel_warm_start() {
    if !std::path::Path::new(LF_PATH).is_dir() {
        println!("SKIP: {LF_PATH} not found");
        return;
    }
    measure("LF COLD", LF_PATH, true, 30).await; // populate cache
    let elapsed = measure("LF WARM", LF_PATH, false, 5).await;
    assert!(elapsed.as_secs() < 5, "warm-start took {elapsed:.2?}");
}
