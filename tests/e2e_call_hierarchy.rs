mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn call_hierarchy_prepare_returns_item() {
    let mut server = TestServer::new().await;
    server
        .open("ch.php", "<?php\nfunction callee(): void {}\ncallee();\n")
        .await;

    let resp = server.prepare_call_hierarchy("ch.php", 1, 9).await;

    assert!(
        resp["error"].is_null(),
        "prepareCallHierarchy error: {resp:?}"
    );
    let items = resp["result"].as_array().expect("array result");
    assert!(!items.is_empty(), "expected at least one CallHierarchyItem");
    assert_eq!(items[0]["name"].as_str().unwrap_or(""), "callee");
}

#[tokio::test]
async fn call_hierarchy_incoming_calls_finds_caller() {
    let mut server = TestServer::new().await;
    let rendered = server
        .check_incoming_calls(
            r#"<?php
function call$0ee(): void {}
function caller(): void { callee(); }
"#,
        )
        .await;
    expect!["caller @ main.php:2"].assert_eq(&rendered);
}

#[tokio::test]
async fn call_hierarchy_outgoing_calls_finds_callee() {
    let mut server = TestServer::new().await;
    let rendered = server
        .check_outgoing_calls(
            r#"<?php
function inner(): void {}
function out$0er(): void { inner(); }
"#,
        )
        .await;
    expect!["inner @ main.php:1"].assert_eq(&rendered);
}
