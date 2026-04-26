mod common;

use common::TestServer;

#[tokio::test]
async fn document_link_returns_array() {
    let mut server = TestServer::new().await;
    server
        .open("dlink.php", "<?php\nrequire_once 'vendor/autoload.php';\n")
        .await;

    let resp = server.document_link("dlink.php").await;

    assert!(resp["error"].is_null(), "documentLink error: {:?}", resp);
    let links = resp["result"]
        .as_array()
        .expect("documentLink must return an array");
    assert!(
        !links.is_empty(),
        "expected at least one link for require_once path"
    );
}

#[tokio::test]
async fn inline_value_returns_array() {
    let mut server = TestServer::new().await;
    server
        .open("inlval.php", "<?php\n$x = 42;\n$y = $x + 1;\n")
        .await;

    let resp = server.inline_value("inlval.php", 2, 0, 2, 10).await;

    assert!(resp["error"].is_null(), "inlineValue error: {:?}", resp);
    let values = resp["result"]
        .as_array()
        .expect("inlineValue must return an array when variables are in range");
    // Line 2 is `$y = $x + 1;` — two variable lookups: $y at col 0 and $x at col 5.
    assert_eq!(values.len(), 2, "expected exactly $y and $x on line 2");
    let names: Vec<&str> = values
        .iter()
        .filter_map(|v| v["variableName"].as_str())
        .collect();
    assert!(
        names.contains(&"y"),
        "expected variable 'y' ($y), got: {:?}",
        names
    );
    assert!(
        names.contains(&"x"),
        "expected variable 'x' ($x), got: {:?}",
        names
    );
}

#[tokio::test]
async fn moniker_returns_no_error() {
    let mut server = TestServer::new().await;
    server
        .open("moniker.php", "<?php\nfunction monikerFn(): void {}\n")
        .await;

    let resp = server.moniker("moniker.php", 1, 9).await;

    assert!(resp["error"].is_null(), "moniker error: {:?}", resp);
    let result = &resp["result"];
    let monikers = result.as_array().expect("expected non-empty moniker array");
    assert_eq!(
        monikers.len(),
        1,
        "expected exactly one moniker for monikerFn"
    );
    assert_eq!(
        monikers[0]["identifier"].as_str().unwrap_or(""),
        "monikerFn",
        "expected moniker identifier 'monikerFn', got: {:?}",
        monikers[0]
    );
    assert_eq!(
        monikers[0]["scheme"].as_str().unwrap_or(""),
        "php",
        "expected moniker scheme 'php'"
    );
}

#[tokio::test]
async fn linked_editing_range_returns_no_error() {
    let mut server = TestServer::new().await;
    server
        .open("linked.php", "<?php\nclass LinkedClass {}\n")
        .await;

    let resp = server.linked_editing_range("linked.php", 1, 6).await;

    assert!(
        resp["error"].is_null(),
        "linkedEditingRange error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected non-null LinkedEditingRanges for class name, got null"
    );
    let ranges = result["ranges"]
        .as_array()
        .expect("expected 'ranges' array in LinkedEditingRanges");
    // `class LinkedClass {}` — the class name is the only occurrence, so exactly one range.
    assert_eq!(
        ranges.len(),
        1,
        "expected exactly one range for LinkedClass"
    );
    // `class ` is 6 chars; `LinkedClass` is 11 chars → cols 6..17.
    assert_eq!(
        ranges[0]["start"],
        serde_json::json!({"line": 1, "character": 6}),
        "range start must point to the L in LinkedClass"
    );
    assert_eq!(
        ranges[0]["end"],
        serde_json::json!({"line": 1, "character": 17}),
        "range end must be after the last char of LinkedClass"
    );
}
