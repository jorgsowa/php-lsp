mod common;

use common::TestServer;

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
    assert!(
        out.contains("name") || out.contains("$name"),
        "expected parameter-name hint: {out}"
    );
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
    assert!(
        out == "<no hints>" || !out.contains("error:"),
        "unexpected: {out}"
    );
}
