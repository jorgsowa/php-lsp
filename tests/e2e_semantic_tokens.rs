mod common;

use common::TestServer;

#[tokio::test]
async fn semantic_tokens_full_returned() {
    let mut server = TestServer::new().await;
    server
        .open(
            "tokens.php",
            "<?php\nfunction tokenized(int $x): int { return $x; }\n",
        )
        .await;

    let resp = server.semantic_tokens_full("tokens.php").await;

    assert!(
        resp["error"].is_null(),
        "semanticTokens/full error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected semanticTokens result, got null"
    );
    let data = result["data"].as_array().expect("data must be an array");
    assert!(
        !data.is_empty(),
        "expected non-empty semantic token data for a file with a typed function"
    );
}

#[tokio::test]
async fn semantic_tokens_range_returns_data() {
    let mut server = TestServer::new().await;
    server
        .open(
            "st_range.php",
            "<?php\nfunction ranged(int $x): int { return $x; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_range("st_range.php", 0, 0, 2, 0)
        .await;

    assert!(
        resp["error"].is_null(),
        "semanticTokens/range error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(!result.is_null(), "expected non-null result");
    let data = result["data"]
        .as_array()
        .expect("expected data array in result");
    assert!(
        !data.is_empty(),
        "expected non-empty token data for a file with typed function"
    );
}

#[tokio::test]
async fn semantic_tokens_full_delta_returns_result() {
    let mut server = TestServer::new().await;
    server
        .open(
            "st_delta.php",
            "<?php\nfunction delta(int $x): int { return $x; }\n",
        )
        .await;

    let full = server.semantic_tokens_full("st_delta.php").await;

    assert!(
        full["error"].is_null(),
        "semanticTokens/full error: {:?}",
        full
    );
    let result_id = full["result"]["resultId"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        !result_id.is_empty(),
        "semanticTokens/full must return a resultId to support delta requests"
    );

    let resp = server
        .semantic_tokens_full_delta("st_delta.php", &result_id)
        .await;

    assert!(
        resp["error"].is_null(),
        "semanticTokens/full/delta error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        result["edits"].is_array() || result["data"].is_array(),
        "expected 'edits' or 'data' in delta result, got: {:?}",
        result
    );
}
