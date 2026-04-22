//! E2E test proof-of-shape for the new harness.
//!
//! Compare with `src/backend.rs::integration::hover_on_opened_document` — the
//! builder collapses ~30 lines of JSON-RPC boilerplate and a `sleep(150ms)`
//! into three statements, and the sync is deterministic.

mod common;

use common::TestServer;

#[tokio::test]
async fn hover_on_opened_document() {
    let mut server = TestServer::new().await;
    server
        .open(
            "test.php",
            "<?php\nfunction greet(string $name): string { return $name; }\n",
        )
        .await;
    let resp = server.hover("test.php", 1, 10).await;

    assert!(resp["error"].is_null(), "hover errored: {:?}", resp);
    assert!(!resp["result"].is_null(), "hover returned null");
    let value = resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(
        value.contains("greet"),
        "hover must show 'greet', got: {value}"
    );
}

#[tokio::test]
async fn hover_with_cursor_marker() {
    let (src, line, character) = common::cursor("<?php\nfunction gr$0eet(): void {}\n");

    let mut server = TestServer::new().await;
    server.open("test.php", &src).await;

    let resp = server.hover("test.php", line, character).await;

    assert!(resp["error"].is_null());
    assert!(!resp["result"].is_null());
    let value = resp["result"]["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(value.contains("greet"), "hover value: {value}");
}
