//! E2E tests for the `workspace/didChangeWorkspaceFolders` handler.
//!
//! Key behaviors exercised:
//! - Added folders trigger a background `scan_workspace` task that indexes PHP files.
//! - After each scan, `send_refresh_requests` fires server→client requests (handled
//!   transparently by the test client's read loop).
//! - Removed folders are removed from `root_paths` but already-indexed docs stay
//!   in the in-memory store.
//! - There is NO `$/php-lsp/indexReady` signal from this handler; we poll with
//!   `workspace_symbols()` instead.

mod common;

use common::TestServer;
use std::time::{Duration, Instant};
use tower_lsp::lsp_types::Url;

/// Poll `workspace_symbols(query)` until a non-empty result arrives or the
/// deadline expires.
async fn poll_until_symbol_present(server: &mut TestServer, query: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let resp = server.workspace_symbols(query).await;
        if resp["result"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {:?} waiting for '{}' to appear in workspace symbols",
            timeout,
            query
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

/// Adding a workspace folder triggers a background scan that indexes the PHP
/// classes inside it.
#[tokio::test]
async fn add_workspace_folder_indexes_php_classes() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    // Create a temp directory with a single PHP file outside the fixture root.
    let tmp = tempfile::tempdir().expect("create TempDir");
    std::fs::write(
        tmp.path().join("ExtraWidget.php"),
        "<?php\nclass ExtraWidget {}\n",
    )
    .expect("write ExtraWidget.php");

    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI from tempdir path")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;

    // Poll until the background scan has indexed ExtraWidget.
    poll_until_symbol_present(&mut server, "ExtraWidget", Duration::from_secs(5)).await;

    let resp = server.workspace_symbols("ExtraWidget").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols
            .iter()
            .any(|s| s["name"].as_str() == Some("ExtraWidget")),
        "ExtraWidget must appear in workspace symbols after adding folder, got: {symbols:?}"
    );
}

/// Adding an empty folder (no PHP files) must not crash the server. The
/// existing workspace contents remain accessible afterwards.
#[tokio::test]
async fn add_empty_workspace_folder_does_not_crash() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    // Empty temp dir — no PHP files.
    let tmp = tempfile::tempdir().expect("create TempDir");
    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI from tempdir path")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;

    // Give the background scan a moment to finish (it completes quickly for
    // an empty directory).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Original fixture symbols must still be reachable — server must be alive.
    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| s["name"].as_str() == Some("User")),
        "User from psr4-mini must still be accessible after adding empty folder, got: {symbols:?}"
    );
}

/// Adding the same folder URI twice is idempotent: the handler's
/// `if !roots.contains(&path)` guard prevents double-indexing, so exactly
/// one symbol named `UniqueGadget` must appear in the results.
#[tokio::test]
async fn add_workspace_folder_idempotent_on_duplicate() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let tmp = tempfile::tempdir().expect("create TempDir");
    std::fs::write(
        tmp.path().join("UniqueGadget.php"),
        "<?php\nclass UniqueGadget {}\n",
    )
    .expect("write UniqueGadget.php");

    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI from tempdir path")
        .to_string();

    // Send the notification twice.
    server.add_workspace_folder(&folder_uri).await;
    server.add_workspace_folder(&folder_uri).await;

    // Wait until at least one result is present.
    poll_until_symbol_present(&mut server, "UniqueGadget", Duration::from_secs(5)).await;

    let resp = server.workspace_symbols("UniqueGadget").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    let count = symbols
        .iter()
        .filter(|s| s["name"].as_str() == Some("UniqueGadget"))
        .count();
    assert_eq!(
        count, 1,
        "UniqueGadget must appear exactly once (not duplicated by double add), got: {symbols:?}"
    );
}

/// Removing a folder removes it from `root_paths`, but already-indexed
/// documents remain in the in-memory store — the server must still answer
/// queries for symbols that were indexed before the removal.
///
/// NOTE: this is a known limitation of the current implementation: removing a
/// folder does NOT evict its docs from the doc store, only from root_paths.
#[tokio::test]
async fn remove_workspace_folder_does_not_crash_and_keeps_indexed_docs() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    // Build the root URI (trim any trailing slash for safety).
    let root_uri = server.uri("").trim_end_matches('/').to_string();

    server.remove_workspace_folder(&root_uri).await;

    // The server must still be responsive after the removal.
    let resp = server.workspace_symbols("User").await;
    assert!(
        resp["error"].is_null(),
        "workspace_symbols must not error after remove_workspace_folder, got: {resp:?}"
    );

    // Docs stay in memory even though root_paths no longer contains the folder.
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| s["name"].as_str() == Some("User")),
        "User must remain accessible after folder removal (docs stay in memory), got: {symbols:?}"
    );
}
