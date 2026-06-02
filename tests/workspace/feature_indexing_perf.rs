//! Indexing performance benchmarks against WordPress.
//!
//! Run with:
//!   cargo test --test workspace indexing_perf -- --ignored --nocapture
//!
//! The tests are gated behind `#[ignore]` because they download nothing —
//! WordPress must be present at /tmp/wordpress. Extract it once with:
//!   curl -L https://wordpress.org/latest.zip -o /tmp/wordpress.zip
//!   unzip /tmp/wordpress.zip -d /tmp/
//!
//! Each run prints timing so you can compare cold vs warm start.

use super::*;

const WP_PATH: &str = "/tmp/wordpress";

fn wp_present() -> bool {
    std::path::Path::new(WP_PATH).is_dir()
}

/// Cold-start index: wipe the cache dir, then measure time to `indexReady`.
/// This is the worst case — every file must be parsed and the result written
/// to the on-disk cache for the first time.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn indexing_perf_cold_start() {
    if !wp_present() {
        println!("SKIP: {WP_PATH} not found");
        return;
    }

    // Wipe the cache so this is a true cold start.
    let cache_dir = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))
        .map(|d| d.join("php-lsp"));
    if let Some(ref d) = cache_dir {
        if d.exists() {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    let start = std::time::Instant::now();
    let mut s = TestServer::with_root(WP_PATH).await;
    s.wait_for_index_ready().await;
    let elapsed = start.elapsed();

    println!("COLD  {elapsed:.2?} — WordPress ({WP_PATH})");
    assert!(
        elapsed.as_secs() < 30,
        "cold-start indexing took {elapsed:.2?}, expected < 30 s"
    );
}

/// Warm-start index: run a cold start first (cache populated), then measure a
/// second start where every file should be served from cache.
#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn indexing_perf_warm_start() {
    if !wp_present() {
        println!("SKIP: {WP_PATH} not found");
        return;
    }

    // Cold pass — populate cache (don't time this one).
    {
        let mut s = TestServer::with_root(WP_PATH).await;
        s.wait_for_index_ready().await;
    }

    // Warm pass — every file comes from the on-disk cache.
    let start = std::time::Instant::now();
    let mut s = TestServer::with_root(WP_PATH).await;
    s.wait_for_index_ready().await;
    let elapsed = start.elapsed();

    println!("WARM  {elapsed:.2?} — WordPress ({WP_PATH})");
    assert!(
        elapsed.as_secs() < 5,
        "warm-start indexing took {elapsed:.2?}, expected < 5 s with cache"
    );
}
