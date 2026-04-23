mod common;

use common::TestServer;

#[tokio::test]
async fn pull_diagnostics_returns_report() {
    let mut server = TestServer::new().await;
    server.open("pull_diag.php", "<?php\n$x = 1;\n").await;

    let resp = server.pull_diagnostics("pull_diag.php").await;

    assert!(
        resp["error"].is_null(),
        "textDocument/diagnostic error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(!result.is_null(), "expected non-null diagnostic report");
    let kind = result["kind"].as_str().unwrap_or("");
    assert!(
        kind == "full" || kind == "unchanged",
        "expected kind 'full' or 'unchanged', got: {:?}",
        kind
    );
}

#[tokio::test]
async fn workspace_diagnostic_returns_report() {
    let mut server = TestServer::new().await;
    server.open("ws_diag.php", "<?php\n$x = 1;\n").await;

    let resp = server.workspace_diagnostic().await;

    assert!(
        resp["error"].is_null(),
        "workspace/diagnostic error: {:?}",
        resp
    );
    let result = &resp["result"];
    let items = result["items"]
        .as_array()
        .expect("expected 'items' array in workspace diagnostic report");
    assert!(
        !items.is_empty(),
        "expected at least one item for the opened file, got empty items"
    );
}
