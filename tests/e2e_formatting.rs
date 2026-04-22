mod common;

use common::TestServer;

#[tokio::test]
async fn formatting_returns_null_or_edits() {
    let mut server = TestServer::new().await;
    server
        .open("fmt.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.formatting("fmt.php").await;

    assert!(resp["error"].is_null(), "formatting error: {:?}", resp);
    assert!(
        resp["result"].is_null() || resp["result"].is_array(),
        "expected null or array, got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn range_formatting_returns_null_or_edits() {
    let mut server = TestServer::new().await;
    server
        .open("rfmt.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.range_formatting("rfmt.php", 0, 0, 2, 0).await;

    assert!(resp["error"].is_null(), "rangeFormatting error: {:?}", resp);
    assert!(
        resp["result"].is_null() || resp["result"].is_array(),
        "expected null or array, got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn on_type_formatting_returns_null_or_edits() {
    let mut server = TestServer::new().await;
    server.open("otfmt.php", "<?php\nif (true) {\n").await;

    let resp = server.on_type_formatting("otfmt.php", 1, 10, "{").await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    assert!(
        resp["result"].is_null() || resp["result"].is_array(),
        "expected null or array, got: {:?}",
        resp["result"]
    );
}
