mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn inlay_hints_for_parameter_names() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function greet(string $name, int $count): void {}
greet('world', 3);
"#,
        )
        .await;
    expect![[r#"
        2:6 name:
        2:15 count:"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_empty_for_file_with_no_calls() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
$x = 1;
$y = 2;
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}
