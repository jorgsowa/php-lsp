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
    assert!(
        !values.is_empty(),
        "expected at least one inline value for $x/$y"
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
    assert!(
        result.is_array() && !result.as_array().unwrap().is_empty(),
        "expected non-empty moniker array, got: {:?}",
        result
    );
    assert_eq!(
        result[0]["identifier"].as_str().unwrap_or(""),
        "monikerFn",
        "expected moniker identifier 'monikerFn', got: {:?}",
        result[0]
    );
    assert_eq!(
        result[0]["scheme"].as_str().unwrap_or(""),
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
    assert!(
        !ranges.is_empty(),
        "expected at least one range for LinkedClass"
    );
}
