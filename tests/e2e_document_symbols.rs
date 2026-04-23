mod common;

use common::TestServer;

#[tokio::test]
async fn document_symbols_lists_functions_and_classes() {
    let mut server = TestServer::new().await;
    server
        .open(
            "syms.php",
            "<?php\nfunction hello(): void {}\nclass World {}\n",
        )
        .await;

    let resp = server.document_symbols("syms.php").await;

    assert!(resp["error"].is_null(), "documentSymbol error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "documentSymbol should return an array, got: {:?}",
        result
    );
    let syms = result.as_array().unwrap();
    assert!(
        syms.len() >= 2,
        "expected at least 2 symbols (hello, World), got {}",
        syms.len()
    );
    let names: Vec<&str> = syms.iter().filter_map(|s| s["name"].as_str()).collect();
    assert!(names.contains(&"hello"), "missing symbol 'hello'");
    assert!(names.contains(&"World"), "missing symbol 'World'");
}

#[tokio::test]
async fn workspace_symbols_returns_matching_items() {
    let mut server = TestServer::new().await;
    server
        .open("wsym.php", "<?php\nclass FuzzyTarget {}\n")
        .await;

    let resp = server.workspace_symbols("FuzzyTarget").await;

    assert!(
        resp["error"].is_null(),
        "workspace/symbol error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(result.is_array(), "expected array, got: {:?}", result);
    let items = result.as_array().unwrap();
    assert!(!items.is_empty(), "expected at least one symbol");
    assert!(
        items
            .iter()
            .any(|s| s["name"].as_str() == Some("FuzzyTarget")),
        "expected FuzzyTarget in results, got: {:?}",
        items
    );
}
