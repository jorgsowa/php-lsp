//! Manual, one-off verification against an arbitrary local workspace outside
//! the repo. NOT committed as a permanent test — path and target file are
//! supplied via environment variables so no external codebase's identity
//! ever appears in source, and no response payload (which could contain a
//! `file://` URI back to that path) is ever printed verbatim.
//!
//! Run with:
//!   MANUAL_VERIFY_ROOT=/path/to/workspace \
//!   MANUAL_VERIFY_FILE=relative/path/to/File.php \
//!   cargo test --test workspace manual_workspace_verify -- --ignored --nocapture

use super::*;
use serde_json::json;
use std::time::Duration;

#[ignore]
#[serial_test::serial]
#[tokio::test]
async fn manual_workspace_verify() {
    let Some(root) = std::env::var("MANUAL_VERIFY_ROOT").ok() else {
        println!("SKIP: set MANUAL_VERIFY_ROOT to run this");
        return;
    };
    let Some(target) = std::env::var("MANUAL_VERIFY_FILE").ok() else {
        println!("SKIP: set MANUAL_VERIFY_FILE (path relative to MANUAL_VERIFY_ROOT) to run this");
        return;
    };
    if !std::path::Path::new(&root).is_dir() {
        println!("SKIP: MANUAL_VERIFY_ROOT is not a directory");
        return;
    }

    println!("PID {}", std::process::id());

    let cache_dir = tempfile::tempdir().expect("cache tempdir");
    let opts = json!({ "cachePath": cache_dir.path().to_str().unwrap() });

    let start = std::time::Instant::now();
    let mut s = TestServer::with_root_and_options(&root, opts).await;
    s.wait_for_index_ready_secs(180).await;
    println!("MILESTONE index_ready {:.2?}", start.elapsed());

    let full_path = std::path::Path::new(&root).join(&target);
    let text = std::fs::read_to_string(&full_path).expect("read MANUAL_VERIFY_FILE");
    s.open(&target, &text).await;

    // ── Regression check: bare-keyword false positive (fixed 2026-08-03) ───
    // Looks for a `final class` occurrence; skipped if the file has none.
    if let Some((_, line, col)) = try_locate(&text, "final class") {
        let def = s.definition(&target, line, col + 1).await;
        let is_empty = def["result"].is_null()
            || (def["result"].is_array() && def["result"].as_array().unwrap().is_empty());
        println!(
            "RESULT final-keyword-no-bogus-jump {}",
            if is_empty { "PASS" } else { "FAIL" }
        );
    } else {
        println!("SKIP final-keyword check: no `final class` in target file");
    }

    // ── Live re-measure of the builtin-type references latency ─────────────
    for needle in ["use Closure", "use ReflectionParameter"] {
        let Some((_, line, col)) = try_locate(&text, needle) else {
            println!("SKIP {needle}: not present in target file");
            continue;
        };
        let uri = s.uri(&target);
        let t0 = std::time::Instant::now();
        let resp = s
            .client()
            .request_with_timeout(
                "textDocument/references",
                json!({
                    "textDocument": {"uri": uri},
                    "position": {"line": line, "character": col + 4},
                    "context": {"includeDeclaration": false},
                }),
                Duration::from_secs(180),
            )
            .await;
        let elapsed = t0.elapsed();
        let n = resp["result"].as_array().map(|a| a.len()).unwrap_or(0);
        println!("RESULT {needle} {n} locations in {elapsed:.2?}");
    }

    println!("MILESTONE requests_done {:.2?}", start.elapsed());

    // ── Let the ambient warm sweep run; poll progress so RSS can be
    //    correlated externally without any ps-polling inside this process.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
    loop {
        if s.wait_for_warm_sweeps(1).await {
            println!("MILESTONE warm_sweep_done {:.2?}", start.elapsed());
            break;
        }
        if std::time::Instant::now() > deadline {
            println!("MILESTONE warm_sweep_timeout {:.2?}", start.elapsed());
            break;
        }
    }

    // ── Idle hold: memory should plateau, not keep climbing, once quiescent.
    println!("MILESTONE idle_start {:.2?}", start.elapsed());
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    println!("MILESTONE idle_end {:.2?}", start.elapsed());

    println!("DONE {:.2?}", start.elapsed());
}

/// `TestServer::locate`, but returns `None` instead of panicking when the
/// needle is absent — this test's needles are optional/best-effort against
/// whatever file the caller points it at.
fn try_locate(text: &str, needle: &str) -> Option<(String, u32, u32)> {
    let byte_pos = text.find(needle)?;
    let before = &text[..byte_pos];
    let line = before.bytes().filter(|b| *b == b'\n').count() as u32;
    let character = before.rsplit('\n').next().unwrap_or("").chars().count() as u32;
    Some((text.to_string(), line, character))
}
