mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn signature_help_inside_function_call() {
    let mut server = TestServer::new().await;
    let rendered = server
        .check_signature_help(
            r#"<?php
function multiply(int $a, int $b): int { return $a * $b; }
multiply(2, $0
"#,
        )
        .await;
    expect!["▶ multiply(int $a, int $b)  @param1"].assert_eq(&rendered);
}
