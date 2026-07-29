//! workspace/didChangeConfiguration: PHP version detection, validation,
//! semantic-token refresh, and repeated calls.

use super::*;

use expect_test::expect;
use serde_json::json;

fn extract_log_message(notif: &serde_json::Value) -> String {
    notif["params"]["message"].as_str().unwrap_or("").to_owned()
}

#[tokio::test]
async fn change_configuration_valid_php_version_is_logged() {
    let mut server = TestServer::new().await;
    let log = server
        .change_configuration(json!({ "phpVersion": "8.3" }))
        .await;
    let msg = extract_log_message(&log);
    expect!["php-lsp: using PHP 8.3 (set by editor)"].assert_eq(&msg);
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

    let warning_msg = server.client().read_notification("window/logMessage").await;
    let warning_text = extract_log_message(&warning_msg);
    expect![[
        r#"php-lsp: unsupported phpVersion "5.6" — valid values: 7.4, 8.0, 8.1, 8.2, 8.3, 8.4, 8.5"#
    ]]
    .assert_eq(&warning_text);

    // Invalid versions skip environment detection and fall straight to the
    // latest stub (PHP_8_5), which resolve_php_version reports as explicit
    // ("set by editor") since from_value already set cfg.php_version — this
    // is deterministic, not environment-dependent.
    let info_msg = server.client().read_notification("window/logMessage").await;
    let info_text = extract_log_message(&info_msg);
    expect!["php-lsp: using PHP 8.5 (set by editor)"].assert_eq(&info_text);
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

    let _log = server.client().read_notification("window/logMessage").await;

    let (refresh_id, _) = server
        .client()
        .expect_server_request("workspace/semanticTokens/refresh")
        .await;
    server
        .client()
        .reply_to_server_request(refresh_id, json!(null))
        .await;
}

#[tokio::test]
async fn change_configuration_can_be_called_twice() {
    let mut server = TestServer::new().await;

    let log1 = server
        .change_configuration(json!({ "phpVersion": "8.1" }))
        .await;
    let msg1 = extract_log_message(&log1);
    expect!["php-lsp: using PHP 8.1 (set by editor)"].assert_eq(&msg1);

    let log2 = server
        .change_configuration(json!({ "phpVersion": "8.3" }))
        .await;
    let msg2 = extract_log_message(&log2);
    expect!["php-lsp: using PHP 8.3 (set by editor)"].assert_eq(&msg2);
}

#[tokio::test]
async fn change_configuration_empty_config_uses_detected_version() {
    let mut server = TestServer::new().await;

    let log = server.change_configuration(json!({})).await;
    let msg = extract_log_message(&log);
    assert!(
        msg.starts_with("php-lsp: using PHP "),
        "expected version log: {msg:?}"
    );
    assert!(
        !msg.contains("set by editor"),
        "empty config must not claim 'set by editor': {msg:?}"
    );
}

/// A runtime PHP-version change must re-populate the workspace file scope,
/// or `workspace_file_paths()` — and everything scoped by it: references,
/// rename, workspace symbols — silently sees an empty workspace afterward.
/// Regression test: `DocumentStore::set_php_version`'s
/// `drop_session_scoped_state` clears `lsp_ws_files` on every real version
/// change, and nothing repopulated it before this fix.
#[tokio::test]
async fn change_configuration_php_version_change_rescans_workspace() {
    let mut server = TestServer::with_fixture_and_options(
        "psr4-mini",
        json!({ "diagnostics": {"enabled": false}, "phpVersion": "8.1" }),
    )
    .await;
    server.wait_for_index_ready().await;

    // Sanity: workspace symbols resolve before the version change.
    server
        .wait_until_symbol_present("User", std::time::Duration::from_secs(5))
        .await;

    server
        .change_configuration(json!({ "phpVersion": "8.3" }))
        .await;

    // Without the fix this call is answered from an emptied workspace and
    // times out; the re-scan the fix adds repopulates it.
    server
        .wait_until_symbol_present("User", std::time::Duration::from_secs(5))
        .await;

    let out = server.snapshot_workspace_symbols("User").await;
    expect![[r#"
        Class       User @ src/Model/User.php:4
        Property    $users @ src/Service/Registry.php:9"#]]
    .assert_eq(&out);
}
