mod common;

use common::TestServer;

#[tokio::test]
async fn will_rename_files_outside_psr4_returns_null() {
    // No PSR-4 map on a rootless server: psr4.file_to_fqn() returns None for
    // both paths, so merged_changes stays empty and the result serializes as null.
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
    // No PSR-4 map, so the handler uses the fallback stub "<?php\n\n".
    // willCreateFiles always inserts a stub, so the result is a WorkspaceEdit
    // with exactly one change entry.
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
    // No PSR-4 map on a rootless server: psr4.file_to_fqn() returns None,
    // so no use-sites are found, merged_changes stays empty, result is null.
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
