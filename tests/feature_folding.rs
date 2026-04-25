mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn folding_ranges_cover_function_body() {
    let mut s = TestServer::new().await;
    let out = s
        .check_folding(
            r#"<?php
function f(): void {
    $a = 1;
    $b = 2;
    $c = 3;
}
"#,
        )
        .await;
    expect!["1..5 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_ranges_cover_class_and_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_folding(
            r#"<?php
class Folded {
    public function method(): void {
        // body
    }
}
"#,
        )
        .await;
    expect![[r#"
        1..5 region
        2..4 region"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_lens_for_function_with_reference() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
function lensed(): void {}
lensed();
"#,
        )
        .await;
    expect!["L1: 1 reference [editor.action.showReferences]"].assert_eq(&out);
}

#[tokio::test]
async fn code_lens_for_class_with_references() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class Widget {}
$w = new Widget();
"#,
        )
        .await;
    expect!["L1: 1 reference [editor.action.showReferences]"].assert_eq(&out);
}
