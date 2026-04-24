mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn inlay_hints_returned_for_function_call() {
    let mut server = TestServer::new().await;
    let rendered = server
        .check_inlay_hints(
            r#"<?php
function divide(int $dividend, int $divisor): float { return $dividend / $divisor; }
divide(10, 2);
"#,
        )
        .await;
    expect![[r#"
        2:7 dividend:
        2:11 divisor:"#]]
    .assert_eq(&rendered);
}

#[tokio::test]
async fn inlay_hint_resolve_returns_hint() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ih_resolve.php",
            "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
        )
        .await;

    let hints_resp = server.inlay_hints("ih_resolve.php", 0, 0, 3, 0).await;
    let hints = hints_resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !hints.is_empty(),
        "expected inlay hints, got {hints_resp:?}"
    );

    let resp = server.inlay_hint_resolve(hints[0].clone()).await;
    assert!(resp["error"].is_null(), "inlayHint/resolve error: {resp:?}");
    assert!(resp["result"].is_object(), "expected resolved hint object");
}
