mod common;

use common::TestServer;

#[tokio::test]
async fn signature_help_inside_function_call() {
    let mut server = TestServer::new().await;
    server
        .open(
            "sig.php",
            "<?php\nfunction multiply(int $a, int $b): int { return $a * $b; }\nmultiply(2, \n",
        )
        .await;

    let resp = server.signature_help("sig.php", 2, 11).await;

    assert!(resp["error"].is_null(), "signatureHelp error: {:?}", resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "expected signatureHelp result, got null");
    let sigs = result["signatures"]
        .as_array()
        .expect("signatures must be an array");
    assert!(!sigs.is_empty(), "expected at least one signature");
    assert_eq!(
        sigs[0]["label"].as_str().unwrap(),
        "multiply(int $a, int $b)",
        "signature label should show the full parameter list"
    );
    assert_eq!(
        result["activeParameter"].as_u64().unwrap(),
        1,
        "cursor after first comma → activeParameter should be 1"
    );
}
