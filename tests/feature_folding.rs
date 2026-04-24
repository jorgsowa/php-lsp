mod common;

use common::TestServer;

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
    assert!(!out.starts_with("error:"), "errored: {out}");
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
    assert!(!out.starts_with("error:"), "errored: {out}");
}
