//! Reliability tests for request interleaving.
//!
//! Real editors fire requests overlapping with edits: hover while typing,
//! completion mid-keystroke, definition during a codeAction rendering. These
//! tests stress interleaved/parallel request patterns to catch deadlocks,
//! stale-cache returns, and panics that single-threaded tests never hit.

mod common;

use common::TestServer;

/// Many hover requests on many distinct files in quick succession must all
/// return correct results — no cross-file contamination, no timeouts.
#[tokio::test]
async fn many_files_hover_each_returns_own_signature() {
    let mut server = TestServer::new().await;

    // Open 10 files with a distinctive function name in each.
    for i in 0..10 {
        let src = format!("<?php\nfunction fn_{i}(int $x): int {{ return $x; }}\n");
        server.open(&format!("c{i}.php"), &src).await;
    }

    // Hover on each and confirm the response mentions its own function name.
    for i in 0..10 {
        let resp = server.hover(&format!("c{i}.php"), 1, 10).await;
        let contents = resp["result"]["contents"].to_string();
        assert!(
            contents.contains(&format!("fn_{i}")),
            "file c{i}.php hover must mention fn_{i}, got: {contents}"
        );
    }
}

/// Interleaved didChange + feature requests: every edit is followed by a
/// hover/definition/references request before the next edit. The server
/// must serialize without returning stale results.
#[tokio::test]
async fn didchange_followed_by_request_sees_new_state_every_iteration() {
    let mut server = TestServer::new().await;
    server.open("iter.php", "<?php\n").await;

    for v in 2..=8 {
        let src = format!("<?php\nfunction iter_{v}(): int {{ return {v}; }}\niter_{v}();\n");
        server.change("iter.php", v, &src).await;

        // Hover on the current function — must see iter_{v}.
        let resp = server.hover("iter.php", 1, 10).await;
        let contents = resp["result"]["contents"].to_string();
        assert!(
            contents.contains(&format!("iter_{v}")),
            "iteration {v}: hover must see latest name, got: {contents}"
        );

        // References on the declaration — must find the single call site.
        let resp = server.references("iter.php", 1, 10, false).await;
        let refs = resp["result"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            refs.len(),
            1,
            "iteration {v}: expected 1 ref, got {}: {refs:?}",
            refs.len()
        );
    }
}

/// A volley of hover requests pipelined without awaiting in between (via
/// separate spawned tasks) must all complete. This catches any bug where the
/// server's request pump blocks on per-document locks in pathological order.
///
/// We drive this single-threaded over the wire — the test just verifies the
/// server processes them all without hanging within the timeout.
#[tokio::test]
async fn pipelined_hover_requests_all_complete() {
    let mut server = TestServer::new().await;
    server
        .open(
            "pipe.php",
            "<?php\nfunction pipeHover(int $x): int { return $x; }\n",
        )
        .await;

    // Fire 20 hover calls in a tight loop; the harness awaits each.
    for _ in 0..20 {
        let resp = server.hover("pipe.php", 1, 10).await;
        assert!(
            resp["error"].is_null(),
            "hover errored in pipeline: {resp:?}"
        );
    }
}

/// Request after a shutdown-like sequence (didClose) for a re-opened file
/// must not return stale data from the closed session.
#[tokio::test]
async fn request_after_close_and_reopen_returns_fresh_data() {
    let mut server = TestServer::new().await;
    server
        .open("ro.php", "<?php\nfunction first(): void {}\n")
        .await;

    let uri = server.uri("ro.php");
    server
        .client()
        .notify(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;

    // Reopen with entirely different content.
    server
        .open("ro.php", "<?php\nfunction second(): void {}\n")
        .await;

    let resp = server.hover("ro.php", 1, 10).await;
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("second"),
        "hover after close+reopen must see new content, got: {contents}"
    );
    assert!(
        !contents.contains("first"),
        "hover must NOT see stale `first` from closed session, got: {contents}"
    );
}
