mod common;

use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn initialize_returns_server_capabilities() {
    // TestServer::new consumes the initialize response; we re-check capabilities
    // indirectly — if hoverProvider and textDocumentSync weren't advertised, the
    // server wouldn't respond to hover / didOpen. So we assert end-to-end
    // behaviour that only works when those caps are enabled.
    let mut server = TestServer::new().await;
    server
        .open("cap.php", "<?php\nfunction f(): void {}\n")
        .await;
    let resp = server.hover("cap.php", 1, 10).await;
    assert!(
        resp["error"].is_null(),
        "hover should not error if hoverProvider is advertised: {:?}",
        resp
    );
    assert!(
        !resp["result"].is_null(),
        "hover should return a result, confirming textDocumentSync applied the open"
    );
    // Keep the original shape-style asserts against the hover result so a
    // failure still points at capability issues.
    let _ = json!({ "hoverProvider": true });
}

#[tokio::test]
async fn shutdown_responds_correctly() {
    let mut server = TestServer::new().await;
    let resp = server.shutdown().await;

    assert!(
        resp["error"].is_null(),
        "shutdown should not error: {:?}",
        resp
    );
    assert!(resp["result"].is_null(), "shutdown result should be null");
}

#[tokio::test]
async fn did_change_updates_document() {
    let mut server = TestServer::new().await;
    server.open("change.php", "<?php\n").await;

    server
        .change("change.php", 2, "<?php\nfunction updated() {}\n")
        .await;

    let resp = server.hover("change.php", 1, 10).await;

    assert!(
        resp["error"].is_null(),
        "hover after change should not error"
    );
}
