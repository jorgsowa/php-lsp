mod common;

use common::TestServer;
use serde_json::json;

#[tokio::test]
async fn symbol_resolve_fills_range_for_open_file() {
    let mut server = TestServer::new().await;
    server
        .open("resolve.php", "<?php\nclass Resolvable {}\n")
        .await;
    let uri = server.uri("resolve.php");

    let symbol = json!({
        "name": "Resolvable",
        "kind": 5,
        "location": { "uri": uri },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;

    assert!(resp["error"].is_null(), "error: {resp:?}");
    let loc = &resp["result"]["location"];
    assert!(
        loc["range"].is_object(),
        "expected range to be filled in for open file: {loc:?}"
    );
    assert_eq!(
        loc["range"]["start"]["line"],
        json!(1),
        "class is on line 1: {loc:?}"
    );
    assert_eq!(
        loc["range"]["start"]["character"],
        json!(6),
        "class name starts at character 6 (after 'class '): {loc:?}"
    );
}

#[tokio::test]
async fn symbol_resolve_unchanged_for_closed_file() {
    let mut server = TestServer::new().await;

    let symbol = json!({
        "name": "ClosedClass",
        "kind": 5,
        "location": { "uri": "file:///nonexistent_closed.php" },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;

    assert!(resp["error"].is_null(), "error: {resp:?}");
    let loc = &resp["result"]["location"];
    assert!(
        !loc.as_object()
            .map(|o| o.contains_key("range"))
            .unwrap_or(false),
        "expected URI-only location for closed file (no range key): {loc:?}"
    );
}

#[tokio::test]
async fn symbol_resolve_passthrough_for_already_resolved_location() {
    let mut server = TestServer::new().await;
    server
        .open("passthrough.php", "<?php\nfunction alreadyResolved() {}\n")
        .await;
    let uri = server.uri("passthrough.php");

    let symbol = json!({
        "name": "alreadyResolved",
        "kind": 12,
        "location": {
            "uri": uri,
            "range": {
                "start": { "line": 1, "character": 9 },
                "end":   { "line": 1, "character": 24 },
            },
        },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;

    assert!(resp["error"].is_null(), "error: {resp:?}");
    let range = &resp["result"]["location"]["range"];
    assert_eq!(range["start"]["line"], json!(1));
    assert_eq!(range["start"]["character"], json!(9));
    assert_eq!(range["end"]["character"], json!(24));
}
