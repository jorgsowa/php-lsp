mod common;

use common::TestServer;

fn has_code(notif: &serde_json::Value, code: &str) -> bool {
    notif["params"]["diagnostics"]
        .as_array()
        .map(|arr| arr.iter().any(|d| d["code"].as_str() == Some(code)))
        .unwrap_or(false)
}

#[tokio::test]
async fn diagnostics_published_on_did_open_for_undefined_function() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("diag_test.php", "<?php\nnonexistent_function();\n")
        .await;

    assert!(
        has_code(&notif, "UndefinedFunction"),
        "expected UndefinedFunction in diagnostics: {:?}",
        notif["params"]["diagnostics"],
    );
}

#[tokio::test]
async fn diagnostics_published_on_did_change_for_undefined_function() {
    let mut server = TestServer::new().await;
    server.open("change_test.php", "<?php\n").await;

    let notif = server
        .change("change_test.php", 2, "<?php\nnonexistent_function();\n")
        .await;

    assert!(
        has_code(&notif, "UndefinedFunction"),
        "expected UndefinedFunction after didChange: {:?}",
        notif["params"]["diagnostics"],
    );
}
