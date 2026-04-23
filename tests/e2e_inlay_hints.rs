mod common;

use common::TestServer;

#[tokio::test]
async fn inlay_hints_returned_for_function_call() {
    let mut server = TestServer::new().await;
    server
        .open(
            "hints.php",
            "<?php\nfunction divide(int $dividend, int $divisor): float { return $dividend / $divisor; }\ndivide(10, 2);\n",
        )
        .await;

    let resp = server.inlay_hints("hints.php", 0, 0, 3, 0).await;

    assert!(resp["error"].is_null(), "inlayHint error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "expected inlayHint array, got: {:?}",
        result
    );
    let hints = result.as_array().unwrap();
    assert_eq!(
        hints.len(),
        2,
        "expected 2 inlay hints (dividend and divisor), got: {:?}",
        hints
    );
    let labels: Vec<&str> = hints.iter().filter_map(|h| h["label"].as_str()).collect();
    assert!(
        labels.contains(&"dividend:"),
        "missing hint 'dividend:', got: {:?}",
        labels
    );
    assert!(
        labels.contains(&"divisor:"),
        "missing hint 'divisor:', got: {:?}",
        labels
    );
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
        "expected inlay hints for add(1, 2) call, got: {:?}",
        hints_resp["result"]
    );

    let resp = server.inlay_hint_resolve(hints[0].clone()).await;

    assert!(
        resp["error"].is_null(),
        "inlayHint/resolve error: {:?}",
        resp
    );
    assert!(resp["result"].is_object(), "expected resolved hint object");
}
