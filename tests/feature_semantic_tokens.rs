//! Semantic token coverage: full, range, delta, and delta-fallback cases.

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

/// Delta request with an unknown `previousResultId` must degrade gracefully
/// to a full-token response — the server must never error out or panic when
/// the client's baseline is stale / unknown (e.g. after a server restart).
#[tokio::test]
async fn semantic_tokens_delta_with_stale_previous_result_id_degrades_to_full() {
    let mut server = TestServer::new().await;
    server
        .open(
            "st_stale.php",
            "<?php\nfunction stale(int $x): int { return $x; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_stale.php", "definitely-not-a-real-id")
        .await;

    assert!(
        resp["error"].is_null(),
        "delta with stale resultId must not error: {resp:?}"
    );
    let result = &resp["result"];
    assert!(!result.is_null(), "expected a result payload, got null");
    let data = result["data"].as_array();
    assert!(
        data.is_some() && !data.unwrap().is_empty(),
        "stale-id delta must fall back to a full token set, got: {result:?}"
    );
}

#[tokio::test]
async fn semantic_tokens_delta_without_baseline_degrades_to_full() {
    let mut server = TestServer::new().await;
    server
        .open(
            "st_noprior.php",
            "<?php\nfunction nobaseline(): int { return 1; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_noprior.php", "0")
        .await;

    assert!(
        resp["error"].is_null(),
        "baseline-less delta must not error: {resp:?}"
    );
    let result = &resp["result"];
    assert!(!result.is_null(), "expected a result, got null");
    assert!(
        result["data"].is_array(),
        "expected full-token fallback (data array), got: {result:?}"
    );
}

/// After `didChange`, requesting delta with the pre-edit resultId must reflect
/// the new content. Either an `edits` diff or a full `data` set is acceptable,
/// but the post-edit token count must exceed the pre-edit count since we added
/// an entire function.
#[tokio::test]
async fn semantic_tokens_delta_after_didchange_reflects_new_content() {
    let mut server = TestServer::new().await;
    server
        .open("st_edit.php", "<?php\nfunction one(): int { return 1; }\n")
        .await;

    let full = server.semantic_tokens_full("st_edit.php").await;
    let pre_id = full["result"]["resultId"]
        .as_str()
        .expect("resultId")
        .to_string();
    let pre_data_len = full["result"]["data"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    server
        .change(
            "st_edit.php",
            2,
            "<?php\nfunction one(): int { return 1; }\nfunction two(): int { return 2; }\n",
        )
        .await;

    let resp = server
        .semantic_tokens_full_delta("st_edit.php", &pre_id)
        .await;
    assert!(resp["error"].is_null(), "delta errored: {resp:?}");
    let result = &resp["result"];

    let got_full = result["data"].is_array();
    let got_edits = result["edits"].is_array();
    assert!(
        got_full || got_edits,
        "delta response must contain `data` or `edits`, got: {result:?}"
    );

    if got_full {
        let post_len = result["data"].as_array().unwrap().len();
        assert!(
            post_len > pre_data_len,
            "post-edit tokens ({post_len}) must exceed pre-edit tokens ({pre_data_len})"
        );
    } else {
        let edits = result["edits"].as_array().unwrap();
        assert!(
            edits
                .iter()
                .any(|e| e["data"].as_array().map(|d| !d.is_empty()).unwrap_or(false)),
            "delta edits must carry new token data, got: {edits:?}"
        );
    }
}
