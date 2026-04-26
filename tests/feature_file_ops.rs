//! File operation stubs: willRenameFiles, willCreateFiles, willDeleteFiles.
//! Covers both rootless servers (no PSR-4 map) and PSR-4-aware fixture workspaces.

mod common;

use common::{TestServer, canonicalize_workspace_edit};
use expect_test::expect;

// ── rootless server (no PSR-4 map) ──────────────────────────────────────────

#[tokio::test]
async fn will_rename_files_outside_psr4_returns_null() {
    let mut server = TestServer::new().await;
    server
        .open("rename_old.php", "<?php\nclass OldClass {}\n")
        .await;

    let old_uri = server.uri("rename_old.php");
    let new_uri = server.uri("rename_new.php");

    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;

    assert!(resp["error"].is_null(), "willRenameFiles error: {:?}", resp);
    assert!(
        resp["result"].is_null(),
        "expected null (no PSR-4 map → no edits), got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn will_create_files_returns_workspace_edit_with_stub() {
    let mut server = TestServer::new().await;
    let uri = server.uri("new_created.php");

    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {:?}", resp);
    assert!(
        resp["result"].is_object(),
        "expected WorkspaceEdit object, got: {:?}",
        resp["result"]
    );
    assert!(
        resp["result"]["changes"].is_object()
            && !resp["result"]["changes"].as_object().unwrap().is_empty(),
        "expected non-empty changes map in WorkspaceEdit, got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn will_delete_files_outside_psr4_returns_null() {
    let mut server = TestServer::new().await;
    server
        .open("to_delete.php", "<?php\nclass ToDelete {}\n")
        .await;

    let uri = server.uri("to_delete.php");

    let resp = server.will_delete_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willDeleteFiles error: {:?}", resp);
    assert!(
        resp["result"].is_null(),
        "expected null (no PSR-4 map → no use-sites to remove), got: {:?}",
        resp["result"]
    );
}

// ── PSR-4-aware stub generation (psr4-mini fixture) ─────────────────────────

#[tokio::test]
async fn will_create_files_psr4_mapped_generates_namespace_stub() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let uri = server.uri("src/Model/Product.php");
    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Model/Product.php
        0:0-0:0 → "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Model;\n\nclass Product\n{\n}\n""#]]
    .assert_eq(&snap);
}

#[tokio::test]
async fn will_create_files_outside_psr4_root_generates_minimal_stub() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let uri = server.uri("scripts/bootstrap.php");
    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    let changes = resp["result"]["changes"]
        .as_object()
        .expect("expected a changes map");
    assert_eq!(changes.len(), 1, "expected exactly one file in changes");

    let edits = changes.values().next().unwrap().as_array().unwrap();
    assert_eq!(edits.len(), 1);
    let new_text = edits[0]["newText"].as_str().unwrap();
    assert_eq!(
        new_text, "<?php\n\n",
        "expected minimal stub for non-PSR-4 path"
    );
}

#[tokio::test]
async fn will_create_files_root_namespace_generates_stub_without_namespace() {
    let mut server = TestServer::with_fixture("psr4-root").await;
    server.wait_for_index_ready().await;

    let uri = server.uri("src/Bootstrap.php");
    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Bootstrap.php
        0:0-0:0 → "<?php\n\ndeclare(strict_types=1);\n\nclass Bootstrap\n{\n}\n""#]]
    .assert_eq(&snap);
}

#[tokio::test]
async fn will_create_files_multiple_files_get_independent_stubs() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let uri_a = server.uri("src/Alpha.php");
    let uri_b = server.uri("src/Beta.php");
    let resp = server.will_create_files(vec![uri_a, uri_b]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Alpha.php
        0:0-0:0 → "<?php\n\ndeclare(strict_types=1);\n\nnamespace App;\n\nclass Alpha\n{\n}\n"

        // src/Beta.php
        0:0-0:0 → "<?php\n\ndeclare(strict_types=1);\n\nnamespace App;\n\nclass Beta\n{\n}\n""#]]
    .assert_eq(&snap);
}
