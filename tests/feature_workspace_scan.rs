//! Workspace scan: didChangeWatchedFiles CREATED/CHANGED/DELETED events,
//! and excludePaths filtering from initializationOptions and .php-lsp.json.

mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::json;
use std::time::{Duration, Instant};

const CREATED: u32 = 1;
const CHANGED: u32 = 2;
const DELETED: u32 = 3;

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
            "timed out after {:?} waiting for '{}' in workspace symbols",
            timeout,
            query
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn poll_until_symbol_absent(server: &mut TestServer, query: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let empty = server.workspace_symbols(query).await["result"]
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

// ── CREATED ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn created_file_becomes_discoverable_via_workspace_symbols() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.snapshot_workspace_symbols("Widget").await;
    expect![[r#"<no symbols>"#]].assert_eq(&pre);

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

// ── CHANGED ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn changed_file_updates_workspace_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.snapshot_workspace_symbols("Greeter").await;
    expect![[r#"Class       Greeter @ src/Service/Greeter.php:6"#]].assert_eq(&pre);

    server.write_file(
        "src/Service/Greeter.php",
        "<?php\nnamespace App\\Service;\n\nclass GreeterUpdated {}\n",
    );
    let uri = server.uri("src/Service/Greeter.php");
    server.did_change_watched_files(vec![(uri, CHANGED)]).await;

    poll_until_symbol_present(&mut server, "GreeterUpdated", Duration::from_secs(3)).await;

    let post = server.snapshot_workspace_symbols("GreeterUpdated").await;
    expect![[r#"Class       GreeterUpdated @ src/Service/Greeter.php:3"#]].assert_eq(&post);

    let gone = server.snapshot_workspace_symbols("Greeter").await;
    expect![[r#"Class       GreeterUpdated @ src/Service/Greeter.php:3"#]].assert_eq(&gone);
}

// ── DELETED ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn deleted_file_symbols_removed_from_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"Class       Registry @ src/Service/Registry.php:6"#]].assert_eq(&pre);

    server.remove_file("src/Service/Registry.php");
    let uri = server.uri("src/Service/Registry.php");
    server.did_change_watched_files(vec![(uri, DELETED)]).await;

    poll_until_symbol_absent(&mut server, "Registry", Duration::from_secs(3)).await;

    let post = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"<no symbols>"#]].assert_eq(&post);
}

// ── excludePaths ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn exclude_paths_honored_by_workspace_scan() {
    let mut server = TestServer::with_fixture_and_options(
        "psr4-mini",
        json!({
            "diagnostics": { "enabled": true },
            "excludePaths": ["src/Service/*"],
        }),
    )
    .await;
    server.wait_for_index_ready().await;

    let resp = server.workspace_symbols("Greeter").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !symbols.iter().any(|s| {
            s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Service/Greeter.php"))
                .unwrap_or(false)
        }),
        "Greeter is in excluded src/Service — must not be indexed, got: {symbols:?}"
    );

    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| {
            s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Model/User.php"))
                .unwrap_or(false)
        }),
        "User is NOT excluded — must still appear in workspace symbols, got: {symbols:?}"
    );
}

#[tokio::test]
async fn php_lsp_json_exclude_paths_honored() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("tests/fixtures/psr4-mini");
    let tmp = tempfile::tempdir().expect("create TempDir");
    fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            let to = dst.join(e.file_name());
            if e.file_type()?.is_dir() {
                copy_dir(&e.path(), &to)?;
            } else {
                std::fs::copy(e.path(), to)?;
            }
        }
        Ok(())
    }
    copy_dir(&source, tmp.path()).unwrap();
    std::fs::write(
        tmp.path().join(".php-lsp.json"),
        r#"{"excludePaths": ["src/Service/*"]}"#,
    )
    .unwrap();

    let mut server = TestServer::with_root(tmp.path()).await;
    server.wait_for_index_ready().await;

    let resp = server.workspace_symbols("Greeter").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.ends_with("src/Service/Greeter.php"))
            .unwrap_or(false)),
        "Greeter is excluded via .php-lsp.json — must not be indexed, got: {symbols:?}"
    );

    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.ends_with("src/Model/User.php"))
            .unwrap_or(false)),
        "User is not excluded — must still be indexed, got: {symbols:?}"
    );
}
