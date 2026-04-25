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
async fn inlay_hint_resolve_returns_same_hint() {
    let mut s = TestServer::new().await;
    s.open(
        "resolve.php",
        "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
    )
    .await;
    let hints_resp = s.inlay_hints("resolve.php", 0, 0, 4, 0).await;
    let hints = hints_resp["result"].as_array().cloned().unwrap_or_default();
    assert!(!hints.is_empty(), "expected inlay hints");
    let resp = s.inlay_hint_resolve(hints[0].clone()).await;
    assert!(resp["error"].is_null(), "inlayHint/resolve error: {resp:?}");
    assert_eq!(
        resp["result"]["label"], hints[0]["label"],
        "resolved label must match original"
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
    expect!["<no hints>"].assert_eq(&out);
}
