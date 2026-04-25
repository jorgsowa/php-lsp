mod common;

use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn change_configuration_valid_php_version_is_logged() {
    let mut server = TestServer::new().await;
    let log = server
        .change_configuration(json!({ "phpVersion": "8.3" }))
        .await;
    let msg = log["params"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("using PHP 8.3"),
        "expected 'using PHP 8.3': {msg:?}"
    );
    assert!(
        msg.contains("set by editor"),
        "expected 'set by editor': {msg:?}"
    );
}

#[tokio::test]
async fn change_configuration_invalid_php_version_logs_warning() {
    let mut server = TestServer::new().await;

    server
        .client()
        .notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": null }),
        )
        .await;
    let (req_id, _) = server
        .client()
        .expect_server_request("workspace/configuration")
        .await;
    server
        .client()
        .reply_to_server_request(req_id, json!([{ "phpVersion": "5.6" }]))
        .await;

    // First log: WARNING about unsupported version
    let warning_msg = server.client().read_notification("window/logMessage").await;
    let warning_text = warning_msg["params"]["message"].as_str().unwrap_or("");
    assert!(
        warning_text.contains("unsupported phpVersion"),
        "expected unsupported version warning: {warning_text:?}"
    );

    // Second log: INFO confirming which version was actually used
    let info_msg = server.client().read_notification("window/logMessage").await;
    let info_text = info_msg["params"]["message"].as_str().unwrap_or("");
    assert!(
        info_text.starts_with("php-lsp: using PHP "),
        "expected PHP version log: {info_text:?}"
    );
}

#[tokio::test]
async fn change_configuration_triggers_semantic_token_refresh() {
    let mut server = TestServer::new().await;

    server
        .client()
        .notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": null }),
        )
        .await;
    let (req_id, _) = server
        .client()
        .expect_server_request("workspace/configuration")
        .await;
    server
        .client()
        .reply_to_server_request(req_id, json!([{ "phpVersion": "8.1" }]))
        .await;

    // Wait for completion log
    let _log = server.client().read_notification("window/logMessage").await;

    // The bug fix adds send_refresh_requests — at least one refresh request must now arrive
    let (refresh_id, _) = server
        .client()
        .expect_server_request("workspace/semanticTokens/refresh")
        .await;
    server
        .client()
        .reply_to_server_request(refresh_id, json!(null))
        .await;
    // Test passes — refresh was sent, proving the bug fix works
}

#[tokio::test]
async fn change_configuration_can_be_called_twice() {
    let mut server = TestServer::new().await;

    let log1 = server
        .change_configuration(json!({ "phpVersion": "8.1" }))
        .await;
    assert!(
        log1["params"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("8.1")
    );

    let log2 = server
        .change_configuration(json!({ "phpVersion": "8.3" }))
        .await;
    assert!(
        log2["params"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("8.3")
    );
}

#[tokio::test]
async fn change_configuration_empty_config_uses_detected_version() {
    let mut server = TestServer::new().await;

    let log = server.change_configuration(json!({})).await;
    let msg = log["params"]["message"].as_str().unwrap_or("");
    assert!(
        msg.starts_with("php-lsp: using PHP "),
        "expected version log: {msg:?}"
    );
    assert!(
        !msg.contains("set by editor"),
        "empty config must not claim 'set by editor': {msg:?}"
    );
}
