mod common;

use common::TestServer;

#[tokio::test]
async fn will_rename_files_returns_no_error() {
    let mut server = TestServer::new().await;
    server
        .open("rename_old.php", "<?php\nclass OldClass {}\n")
        .await;

    let old_uri = server.uri("rename_old.php");
    let new_uri = server.uri("rename_new.php");

    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;

    assert!(resp["error"].is_null(), "willRenameFiles error: {:?}", resp);
    assert!(
        resp["result"].is_null() || resp["result"].is_object(),
        "expected null or WorkspaceEdit, got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn will_create_files_returns_no_error() {
    let mut server = TestServer::new().await;
    let uri = server.uri("new_created.php");

    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {:?}", resp);
    assert!(
        resp["result"].is_null() || resp["result"].is_object(),
        "expected null or WorkspaceEdit, got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn will_delete_files_returns_no_error() {
    let mut server = TestServer::new().await;
    server
        .open("to_delete.php", "<?php\nclass ToDelete {}\n")
        .await;

    let uri = server.uri("to_delete.php");

    let resp = server.will_delete_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willDeleteFiles error: {:?}", resp);
    assert!(
        resp["result"].is_null() || resp["result"].is_object(),
        "expected null or WorkspaceEdit, got: {:?}",
        resp["result"]
    );
}
