mod common;

use common::TestServer;

#[tokio::test]
async fn completion_after_initialize() {
    let mut server = TestServer::new().await;
    server.open("comp.php", "<?php\n").await;

    let resp = server.completion("comp.php", 1, 0).await;

    assert!(resp["error"].is_null(), "completion error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array() || result.get("items").is_some() || result.is_null(),
        "unexpected completion shape: {:?}",
        result
    );
}
