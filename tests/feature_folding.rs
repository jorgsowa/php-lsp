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
    // Function body spans lines 1–5 and is exposed as a "region" fold.
    expect!["1..5 region"].assert_eq(&out);
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
    assert!(
        out.contains("reference"),
        "expected reference count lens: {out}"
    );
}
