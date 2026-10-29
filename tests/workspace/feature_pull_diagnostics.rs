use super::*;
use expect_test::expect;
use serde_json::json;

#[tokio::test]
async fn pull_diagnostics_returns_parse_errors() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
class {
"#,
    )
    .await;

    let resp = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()}
            }),
        )
        .await;

    let out = render_pull_diagnostics(&resp);
    expect![[r#"
        1:6-1:7 [1] SyntaxError: expected class name, found '{'
        2:0-2:0 [1] SyntaxError: expected '}', found end of file"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn pull_diagnostics_includes_semantic_errors() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
function foo() {
    echo undefined_function();
}
"#,
    )
    .await;

    let resp = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()}
            }),
        )
        .await;

    expect!["2:9-2:29 [1] UndefinedFunction: Function undefined_function() is not defined"]
        .assert_eq(&render_pull_diagnostics(&resp));
}

#[tokio::test]
async fn pull_diagnostics_empty_on_valid_code() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
function foo(): int {
    return 42;
}
"#,
    )
    .await;

    let resp = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()}
            }),
        )
        .await;

    expect!["<empty>"].assert_eq(&render_pull_diagnostics(&resp));
}

#[tokio::test]
async fn pull_diagnostics_result_id_stable() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
$x = 1;
"#,
    )
    .await;

    // Request diagnostics twice
    let resp1 = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()}
            }),
        )
        .await;

    let resp2 = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()}
            }),
        )
        .await;

    let id1 = resp1["result"]["resultId"].clone();
    let id2 = resp2["result"]["resultId"].clone();

    // Same content should produce same result_id for caching
    assert_eq!(id1, id2, "result_id should be stable for same content");
}

/// When the client's `previousResultId` matches the current content, the
/// spec allows (and clients rely on) an `unchanged` report instead of
/// resending the same diagnostics. `handle_workspace_diagnostic` already did
/// this; `handle_diagnostic` (single-document pull) did not.
#[tokio::test]
async fn pull_diagnostics_unchanged_when_previous_result_id_matches() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
function foo() {
    echo undefined_function();
}
"#,
    )
    .await;

    let resp1 = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()}
            }),
        )
        .await;
    assert_eq!(resp1["result"]["kind"], "full");
    let result_id = resp1["result"]["resultId"].as_str().unwrap().to_owned();

    let resp2 = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()},
                "previousResultId": result_id,
            }),
        )
        .await;
    assert_eq!(
        resp2["result"]["kind"], "unchanged",
        "expected an unchanged report when previousResultId matches: {resp2:?}"
    );
    assert_eq!(resp2["result"]["resultId"], result_id);
    assert!(
        resp2["result"]["items"].is_null(),
        "an unchanged report must not carry an items array: {resp2:?}"
    );
}

/// Contrast: a stale `previousResultId` (content changed since) must still
/// get a full report, not an `unchanged` one.
#[tokio::test]
async fn pull_diagnostics_full_when_previous_result_id_stale() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open("test.php", "<?php\n$x = 1;\n").await;

    s.client()
        .notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri.to_string(), "version": 2},
                "contentChanges": [{"text": "<?php\nundefined_function();\n"}]
            }),
        )
        .await;

    let resp = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()},
                "previousResultId": "stale-id-from-before-the-edit",
            }),
        )
        .await;
    assert_eq!(resp["result"]["kind"], "full");
    expect!["1:0-1:20 [1] UndefinedFunction: Function undefined_function() is not defined"]
        .assert_eq(&render_pull_diagnostics(&resp));
}

/// A client that declares pull support must not also receive pushes, or each
/// diagnostic is shown twice.
#[tokio::test]
async fn pull_capable_client_gets_no_push_diagnostics() {
    let (mut s, _) = TestServer::new_with_client_capabilities(json!({
        "textDocument": { "diagnostic": { "dynamicRegistration": false } }
    }))
    .await;
    let uri = s.uri("test.php");
    let text = "<?php\nundefined_function();\n";
    s.client()
        .notify(
            "textDocument/didOpen",
            json!({
                "textDocument": {"uri": uri, "languageId": "php", "version": 1, "text": text}
            }),
        )
        .await;

    let resp = s.pull_diagnostics("test.php").await;
    expect!["1:0-1:20 [1] UndefinedFunction: Function undefined_function() is not defined"]
        .assert_eq(&render_pull_diagnostics(&resp));

    s.client()
        .notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri, "version": 2},
                "contentChanges": [{"text": "<?php\nother_undefined();\n"}]
            }),
        )
        .await;
    s.client()
        .notify(
            "textDocument/didSave",
            json!({"textDocument": {"uri": uri}}),
        )
        .await;
    let pushed = s
        .client()
        .drain_publish_diagnostics_uris(std::time::Duration::from_millis(500))
        .await;
    assert!(pushed.is_empty(), "unexpected pushes: {pushed:?}");
}
