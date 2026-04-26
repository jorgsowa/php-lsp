//! Workspace folders and watched-file notifications: add/remove folders,
//! did{Create,Delete,Rename}Files, and edge cases from workspace-scan path.

mod common;

use common::TestServer;
use serde_json::json;
use std::time::{Duration, Instant};
use tower_lsp::lsp_types::Url;

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

async fn poll_until_symbol_uri_contains(
    server: &mut TestServer,
    query: &str,
    needle: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let found = server.workspace_symbols(query).await["result"]
            .as_array()
            .map(|a| {
                a.iter().any(|s| {
                    s["location"]["uri"]
                        .as_str()
                        .map(|u| u.contains(needle))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if found {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {:?} waiting for '{}' with URI containing '{}'",
            timeout,
            query,
            needle
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

// ── workspace/didChangeWorkspaceFolders ───────────────────────────────────────

#[tokio::test]
async fn add_workspace_folder_indexes_php_classes() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let tmp = tempfile::tempdir().expect("create TempDir");
    std::fs::write(
        tmp.path().join("ExtraWidget.php"),
        "<?php\nclass ExtraWidget {}\n",
    )
    .expect("write ExtraWidget.php");

    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;
    poll_until_symbol_present(&mut server, "ExtraWidget", Duration::from_secs(5)).await;

    let resp = server.workspace_symbols("ExtraWidget").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols
            .iter()
            .any(|s| s["name"].as_str() == Some("ExtraWidget")),
        "ExtraWidget must appear after adding folder, got: {symbols:?}"
    );
}

#[tokio::test]
async fn add_empty_workspace_folder_does_not_crash() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let tmp = tempfile::tempdir().expect("create TempDir");
    let folder_uri = Url::from_file_path(tmp.path())
        .expect("valid file URI")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| s["name"].as_str() == Some("User")),
        "User from psr4-mini must still be accessible, got: {symbols:?}"
    );
}

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
        .expect("valid file URI")
        .to_string();

    server.add_workspace_folder(&folder_uri).await;
    server.add_workspace_folder(&folder_uri).await;
    poll_until_symbol_present(&mut server, "UniqueGadget", Duration::from_secs(5)).await;

    let resp = server.workspace_symbols("UniqueGadget").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    let count = symbols
        .iter()
        .filter(|s| s["name"].as_str() == Some("UniqueGadget"))
        .count();
    assert_eq!(
        count, 1,
        "UniqueGadget must appear exactly once (not duplicated), got: {symbols:?}"
    );
}

#[tokio::test]
async fn remove_workspace_folder_does_not_crash_and_keeps_indexed_docs() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let root_uri = server.uri("").trim_end_matches('/').to_string();
    server.remove_workspace_folder(&root_uri).await;

    let resp = server.workspace_symbols("User").await;
    assert!(
        resp["error"].is_null(),
        "workspace_symbols must not error after remove_workspace_folder: {resp:?}"
    );
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| s["name"].as_str() == Some("User")),
        "User must remain accessible after folder removal (docs stay in memory): {symbols:?}"
    );
}

// ── workspace-scan edge cases ─────────────────────────────────────────────────

#[tokio::test]
async fn workspace_without_composer_json_still_works() {
    let mut server = TestServer::with_fixture("no-composer").await;
    server.wait_for_index_ready().await;

    let (text, line, ch) = server.locate("src/standalone.php", "standalone", 0);
    server.open("src/standalone.php", &text).await;
    let resp = server.hover("src/standalone.php", line, ch).await;
    assert!(resp["error"].is_null(), "hover errored: {resp:?}");
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("standalone") && contents.contains("int"),
        "hover must work without composer.json, got: {contents}"
    );
}

#[tokio::test]
async fn nonexistent_psr4_dir_does_not_crash_server() {
    let mut server = TestServer::with_fixture("missing-psr4-dir").await;
    server.wait_for_index_ready().await;

    let resp = server.workspace_symbols("Alive").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| {
            s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Present/Alive.php"))
                .unwrap_or(false)
        }),
        "Alive in existing PSR-4 root must be indexed despite missing sibling dir: {symbols:?}"
    );

    let (text, _, _) = server.locate("src/Present/Alive.php", "<?php", 0);
    server.open("src/Present/Alive.php", &text).await;
    let resp = server.document_symbols("src/Present/Alive.php").await;
    assert!(
        resp["error"].is_null(),
        "documentSymbol errored with missing PSR-4 dir: {resp:?}"
    );
}

#[tokio::test]
async fn malformed_composer_json_does_not_crash_server() {
    let mut server = TestServer::with_fixture("broken-composer").await;
    server.wait_for_index_ready().await;

    let (text, _, _) = server.locate("src/Thing.php", "<?php", 0);
    server.open("src/Thing.php", &text).await;

    let resp = server.document_symbols("src/Thing.php").await;
    assert!(
        resp["error"].is_null(),
        "documentSymbol errored after malformed composer: {resp:?}"
    );
    let result = &resp["result"];
    let has_thing = result
        .as_array()
        .map(|arr| {
            arr.iter().any(|s| {
                s["name"].as_str() == Some("Thing") || s["name"].as_str() == Some("App\\Thing")
            })
        })
        .unwrap_or(false);
    assert!(
        has_thing,
        "expected `Thing` in document symbols despite broken composer, got: {result:?}"
    );
}

// ── workspace/didCreateFiles / didDeleteFiles / didRenameFiles ────────────────

#[tokio::test]
async fn did_rename_files_updates_index_to_new_path() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Entity/User.php");

    let (content, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.write_file("src/Entity/User.php", &content);
    server.remove_file("src/Model/User.php");

    server
        .did_rename_files(vec![(old_uri.clone(), new_uri.clone())])
        .await;

    poll_until_symbol_uri_contains(
        &mut server,
        "User",
        "Entity/User.php",
        Duration::from_secs(3),
    )
    .await;

    let post = server.workspace_symbols("User").await;
    let post_symbols = post["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !post_symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.contains("Model/User.php"))
            .unwrap_or(false)),
        "old URI must not appear after rename: {post_symbols:?}"
    );
    assert!(
        post_symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.contains("Entity/User.php"))
            .unwrap_or(false)),
        "new URI must appear after rename: {post_symbols:?}"
    );
}

#[tokio::test]
async fn did_create_files_adds_new_class_to_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.workspace_symbols("OrderRepo").await;
    assert!(
        pre["result"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "OrderRepo must not be indexed before creation"
    );

    server.write_file(
        "src/Repository/OrderRepo.php",
        "<?php\nnamespace App\\Repository;\nclass OrderRepo {}\n",
    );
    let new_uri = server.uri("src/Repository/OrderRepo.php");
    server.did_create_files(vec![new_uri]).await;

    poll_until_symbol_present(&mut server, "OrderRepo", Duration::from_secs(3)).await;

    let post = server.workspace_symbols("OrderRepo").await;
    let symbols = post["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !symbols.is_empty(),
        "OrderRepo must be discoverable after did_create_files: {symbols:?}"
    );
}

#[tokio::test]
async fn did_delete_files_removes_class_and_clears_diagnostics() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let (content, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.open("src/Model/User.php", &content).await;

    let uri = server.uri("src/Model/User.php");
    server.remove_file("src/Model/User.php");

    let results = server.did_delete_files(vec![uri]).await;

    let diag_notif = &results[0];
    let diagnostics = diag_notif["params"]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        diagnostics.is_empty(),
        "publishDiagnostics after deletion must be empty, got: {diagnostics:?}"
    );

    poll_until_symbol_absent(&mut server, "User", Duration::from_secs(3)).await;

    let post = server.workspace_symbols("User").await;
    assert!(
        post["result"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true),
        "User must be removed from workspace symbols after deletion: {:?}",
        post["result"]
    );
}

// ── didChangeWatchedFiles edge cases ──────────────────────────────────────────

#[tokio::test]
async fn changed_event_does_not_overwrite_open_editor_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("editor.php"),
        "<?php\nfunction diskVersion(): void {}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(tmp.path()).await;
    server
        .open("editor.php", "<?php\nfunction editorVersion(): void {}\n")
        .await;

    let uri = server.uri("editor.php");
    server.did_change_watched_files(vec![(uri, CHANGED)]).await;

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
}

#[tokio::test]
async fn batch_changes_all_applied() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    server.write_file(
        "src/Service/Alpha.php",
        "<?php\nnamespace App\\Service;\n\nclass Alpha {}\n",
    );
    server.write_file(
        "src/Service/Beta.php",
        "<?php\nnamespace App\\Service;\n\nclass Beta {}\n",
    );
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

    poll_until_symbol_present(&mut server, "Alpha", Duration::from_secs(3)).await;
    poll_until_symbol_present(&mut server, "Beta", Duration::from_secs(3)).await;
    poll_until_symbol_absent(&mut server, "Registry", Duration::from_secs(3)).await;

    let alpha_out = server.snapshot_workspace_symbols("Alpha").await;
    expect![[r#"Class       Alpha @ src/Service/Alpha.php:3"#]].assert_eq(&alpha_out);

    let beta_out = server.snapshot_workspace_symbols("Beta").await;
    expect![[r#"Class       Beta @ src/Service/Beta.php:3"#]].assert_eq(&beta_out);

    let registry_out = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"<no symbols>"#]].assert_eq(&registry_out);
}
