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
    expect![[r#"
        Class Greeter @L1
          Method hello @L2
          Method bye @L3
        Function top_level @L5"#]]
    .assert_eq(&out);
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
    expect![[r#"
        Enum Status @L1
          EnumMember Active @L2
          EnumMember Inactive @L3"#]]
    .assert_eq(&out);
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
    expect![""].assert_eq(&out);
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
    expect!["Class       MagicRegistry @ main.php:1"].assert_eq(&out);
}
