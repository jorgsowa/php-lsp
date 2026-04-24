mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn document_symbols_lists_functions_and_classes() {
    let mut server = TestServer::new().await;
    let rendered = server
        .check_document_symbols(
            r#"<?php
function hello(): void {}
class World {}
"#,
        )
        .await;
    expect![[r#"
        Function hello @L1
        Class World @L2"#]]
    .assert_eq(&rendered);
}

#[tokio::test]
async fn workspace_symbols_returns_matching_items() {
    let mut server = TestServer::new().await;
    let rendered = server
        .check_workspace_symbols(
            r#"<?php
class FuzzyTarget {}
"#,
            "FuzzyTarget",
        )
        .await;
    assert!(
        rendered.contains("FuzzyTarget"),
        "expected FuzzyTarget in:\n{rendered}"
    );
}
