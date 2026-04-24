//! Document + workspace symbol coverage.

mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn document_symbols_outline() {
    let mut s = TestServer::new().await;
    let out = s
        .check_document_symbols(
            r#"<?php
class Greeter {
    public function hello(): string { return 'hi'; }
    public function bye(): void {}
}
function top_level(): void {}
"#,
        )
        .await;
    assert!(out.contains("Greeter"));
    assert!(out.contains("hello"));
    assert!(out.contains("bye"));
    assert!(out.contains("top_level"));
}

#[tokio::test]
async fn document_symbols_nested_enum() {
    let mut s = TestServer::new().await;
    let out = s
        .check_document_symbols(
            r#"<?php
enum Status {
    case Active;
    case Inactive;
}
"#,
        )
        .await;
    assert!(out.contains("Status"));
}

#[ignore = "php-lsp gap: interface method declarations missing from document symbols"]
#[tokio::test]
async fn document_symbols_interface() {
    let mut s = TestServer::new().await;
    let out = s
        .check_document_symbols(
            r#"<?php
interface Writable {
    public function write(): void;
}
"#,
        )
        .await;
    assert!(out.contains("Writable"));
    assert!(out.contains("write"));
}

#[tokio::test]
async fn workspace_symbols_finds_class_by_query() {
    let mut s = TestServer::new().await;
    let out = s
        .check_workspace_symbols(
            r#"<?php
class MagicRegistry {}
function abracadabra(): void {}
"#,
            "MagicReg",
        )
        .await;
    assert!(
        out.contains("MagicRegistry"),
        "expected MagicRegistry in workspace symbols, got: {out}"
    );
}
