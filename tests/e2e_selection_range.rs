mod common;

use common::TestServer;

#[tokio::test]
async fn selection_range_expands_from_position() {
    let mut server = TestServer::new().await;
    server
        .open(
            "sel.php",
            "<?php\nfunction select(int $x): int { return $x + 1; }\n",
        )
        .await;

    let resp = server.selection_range("sel.php", vec![(1, 30)]).await;

    assert!(resp["error"].is_null(), "selectionRange error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "selectionRange must return an array: {:?}",
        result
    );
    let items = result.as_array().unwrap();
    assert!(
        !items.is_empty(),
        "expected at least one selectionRange entry"
    );

    let mut node = &items[0];
    loop {
        let end_char = node["range"]["end"]["character"].as_u64().unwrap_or(0);
        assert_ne!(
            end_char,
            u32::MAX as u64,
            "selectionRange end character must not be u32::MAX — use real line length"
        );
        if node["parent"].is_null() || !node["parent"].is_object() {
            break;
        }
        node = &node["parent"];
    }
}
