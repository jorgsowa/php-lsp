//! E2E tests for `workspace/didChangeWatchedFiles`.
//!
//! This is how the server learns about file-system events that originate
//! outside the editor: `git checkout`, `composer install`, file saves from
//! another tool.  The three FileChangeType cases each have distinct semantics:
//!
//!   1 = CREATED  — read file from disk, add to index (unless already open)
//!   2 = CHANGED  — read file from disk, update index (unless already open)
//!   3 = DELETED  — remove from index, cross-file features stop resolving it
//!
//! The "unless already open" guard (`index_from_doc_if_not_open`) is tested
//! explicitly to catch regressions where an external write would clobber the
//! editor's in-memory version.

mod common;

use common::TestServer;
use expect_test::expect;
use std::time::{Duration, Instant};

// ── synchronization helpers ────────────────────────────────────────────────

/// Poll `workspace/symbol` until `query` returns at least one result.
/// Panics with a clear message after `timeout`.
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

/// Poll `workspace/symbol` until `query` returns zero results.
async fn poll_until_symbol_absent(server: &mut TestServer, query: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let resp = server.workspace_symbols(query).await;
        let empty = resp["result"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true);
        if empty {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {:?} waiting for '{}' to disappear from workspace symbols",
            timeout,
            query
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

const CREATED: u32 = 1;
const CHANGED: u32 = 2;
const DELETED: u32 = 3;

// ── CREATED ────────────────────────────────────────────────────────────────

/// A new PHP file written to disk and reported as CREATED must become
/// discoverable via `workspace/symbol` without reopening the server.
#[tokio::test]
async fn created_file_becomes_discoverable_via_workspace_symbols() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    // Widget doesn't exist yet.
    let pre = server.snapshot_workspace_symbols("Widget").await;
    expect![[r#"<no symbols>"#]].assert_eq(&pre);

    // Write the file to disk, then tell the server it was created.
    server.write_file(
        "src/Service/Widget.php",
        "<?php\nnamespace App\\Service;\n\nclass Widget {}\n",
    );
    let uri = server.uri("src/Service/Widget.php");
    server.did_change_watched_files(vec![(uri, CREATED)]).await;

    poll_until_symbol_present(&mut server, "Widget", Duration::from_secs(3)).await;

    let post = server.snapshot_workspace_symbols("Widget").await;
    expect![[r#"Class       Widget @ src/Service/Widget.php:3"#]].assert_eq(&post);
}

/// A CREATED event for a deeply-nested path (new sub-package directory) must
/// work just as well as one in an existing directory.
#[tokio::test]
async fn created_file_in_new_subdirectory_is_indexed() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    server.write_file(
        "src/Queue/Job.php",
        "<?php\nnamespace App\\Queue;\n\nclass Job {}\n",
    );
    let uri = server.uri("src/Queue/Job.php");
    server.did_change_watched_files(vec![(uri, CREATED)]).await;

    poll_until_symbol_present(&mut server, "Job", Duration::from_secs(3)).await;

    let out = server.snapshot_workspace_symbols("Job").await;
    expect![[r#"Class       Job @ src/Queue/Job.php:3"#]].assert_eq(&out);
}

/// CREATED for a path that does not exist on disk must not crash the server.
/// The server tries to read the file, fails silently, and continues processing.
#[tokio::test]
async fn created_for_nonexistent_path_does_not_crash() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let ghost_uri = server.uri("src/Service/Ghost.php");
    // Do NOT write anything to disk — the URI points to a non-existent file.
    server
        .did_change_watched_files(vec![(ghost_uri, CREATED)])
        .await;

    // Server must still be alive and answer requests.
    let resp = server.workspace_symbols("User").await;
    assert!(
        resp["error"].is_null(),
        "server must survive CREATED for a non-existent path: {resp:?}"
    );
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| s["name"].as_str() == Some("User")),
        "pre-existing User class must still be indexed after failed CREATED: {symbols:?}"
    );

    // Prove the handler is still processing subsequent events — not just alive.
    server.write_file(
        "src/Survivor.php",
        "<?php\nnamespace App;\n\nclass Survivor {}\n",
    );
    let real_uri = server.uri("src/Survivor.php");
    server
        .did_change_watched_files(vec![(real_uri, CREATED)])
        .await;
    poll_until_symbol_present(&mut server, "Survivor", Duration::from_secs(3)).await;
}

// ── CHANGED ────────────────────────────────────────────────────────────────

/// A file modified outside the editor and reported as CHANGED must update
/// the workspace index so the new symbol is discoverable.
#[tokio::test]
async fn changed_file_updates_workspace_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    // Verify Greeter is indexed under its original name.
    let pre = server.snapshot_workspace_symbols("Greeter").await;
    expect![[r#"Class       Greeter @ src/Service/Greeter.php:6"#]].assert_eq(&pre);

    // Overwrite the file with a renamed class.
    server.write_file(
        "src/Service/Greeter.php",
        "<?php\nnamespace App\\Service;\n\nclass GreeterUpdated {}\n",
    );
    let uri = server.uri("src/Service/Greeter.php");
    server.did_change_watched_files(vec![(uri, CHANGED)]).await;

    poll_until_symbol_present(&mut server, "GreeterUpdated", Duration::from_secs(3)).await;

    let post = server.snapshot_workspace_symbols("GreeterUpdated").await;
    expect![[r#"Class       GreeterUpdated @ src/Service/Greeter.php:3"#]].assert_eq(&post);

    // The old "Greeter" class must be gone from the index (no entry at line 6).
    // workspace_symbols("Greeter") may still return GreeterUpdated via prefix match,
    // but the original class definition must not appear — catches append-vs-replace bugs.
    let gone = server.snapshot_workspace_symbols("Greeter").await;
    expect![[r#"Class       GreeterUpdated @ src/Service/Greeter.php:3"#]].assert_eq(&gone);
}

/// When a file is currently open in the editor, a CHANGED event must NOT
/// overwrite the editor's in-memory version (`index_from_doc_if_not_open`).
/// The server must continue to serve the editor's text for hover/definition.
#[tokio::test]
async fn changed_event_does_not_overwrite_open_editor_file() {
    // Need a real root so write_file works and the server can resolve disk paths.
    let tmp = tempfile::tempdir().unwrap();
    // Seed the on-disk file so the URI resolves to a real path.
    std::fs::write(
        tmp.path().join("editor.php"),
        "<?php\nfunction diskVersion(): void {}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(tmp.path()).await;

    // Open the file in the editor with a *different* in-memory version.
    server
        .open("editor.php", "<?php\nfunction editorVersion(): void {}\n")
        .await;

    // Tell the server the on-disk file changed.
    let uri = server.uri("editor.php");
    server.did_change_watched_files(vec![(uri, CHANGED)]).await;

    // Hover must still return the editor's version, not the disk version.
    let resp = server.hover("editor.php", 1, 10).await;
    assert!(resp["error"].is_null(), "hover errored: {resp:?}");
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("editorVersion"),
        "hover must reflect the editor's version after CHANGED event, got: {contents}"
    );
    assert!(
        !contents.contains("diskVersion"),
        "hover must NOT reflect the on-disk version — open file guard failed, got: {contents}"
    );

    drop(tmp);
}

// ── DELETED ────────────────────────────────────────────────────────────────

/// A file reported as DELETED must be removed from the index so its symbols
/// no longer appear in workspace-symbol queries.
#[tokio::test]
async fn deleted_file_symbols_removed_from_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    // Registry is indexed at startup.
    let pre = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"Class       Registry @ src/Service/Registry.php:6"#]].assert_eq(&pre);

    // Remove from disk and notify the server.
    server.remove_file("src/Service/Registry.php");
    let uri = server.uri("src/Service/Registry.php");
    server.did_change_watched_files(vec![(uri, DELETED)]).await;

    poll_until_symbol_absent(&mut server, "Registry", Duration::from_secs(3)).await;

    let post = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"<no symbols>"#]].assert_eq(&post);
}

/// DELETED for a URI that was never opened or indexed must not crash the server.
#[tokio::test]
async fn deleted_never_indexed_file_does_not_crash() {
    let mut server = TestServer::new().await;

    let ghost_uri = server.uri("never_existed.php");
    server
        .did_change_watched_files(vec![(ghost_uri, DELETED)])
        .await;

    // Server must still respond to requests.
    let resp = server.workspace_symbols("").await;
    assert!(
        resp["error"].is_null(),
        "server must survive DELETED for an unknown URI: {resp:?}"
    );
}

// ── batch ──────────────────────────────────────────────────────────────────

/// Multiple changes in a single notification must all be applied. This covers
/// the `composer install` pattern where many files land simultaneously.
#[tokio::test]
async fn batch_changes_all_applied() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    // Write two new files.
    server.write_file(
        "src/Service/Alpha.php",
        "<?php\nnamespace App\\Service;\n\nclass Alpha {}\n",
    );
    server.write_file(
        "src/Service/Beta.php",
        "<?php\nnamespace App\\Service;\n\nclass Beta {}\n",
    );
    // Delete an existing file in the same batch.
    server.remove_file("src/Service/Registry.php");

    let alpha_uri = server.uri("src/Service/Alpha.php");
    let beta_uri = server.uri("src/Service/Beta.php");
    let registry_uri = server.uri("src/Service/Registry.php");

    server
        .did_change_watched_files(vec![
            (alpha_uri, CREATED),
            (beta_uri, CREATED),
            (registry_uri, DELETED),
        ])
        .await;

    // Both created files must become discoverable.
    poll_until_symbol_present(&mut server, "Alpha", Duration::from_secs(3)).await;
    poll_until_symbol_present(&mut server, "Beta", Duration::from_secs(3)).await;

    let alpha_out = server.snapshot_workspace_symbols("Alpha").await;
    expect![[r#"Class       Alpha @ src/Service/Alpha.php:3"#]].assert_eq(&alpha_out);

    let beta_out = server.snapshot_workspace_symbols("Beta").await;
    expect![[r#"Class       Beta @ src/Service/Beta.php:3"#]].assert_eq(&beta_out);

    // Deleted file must be gone.
    poll_until_symbol_absent(&mut server, "Registry", Duration::from_secs(3)).await;

    let registry_out = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"<no symbols>"#]].assert_eq(&registry_out);
}
