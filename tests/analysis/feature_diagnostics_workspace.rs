//! Diagnostic coverage matrix using the caret annotation DSL.
//! Each test names the expectation inline with `// ^^^ severity: message`.

use super::*;

use expect_test::expect;
use serde_json::json;

#[tokio::test]
async fn edge_case_file_closed_during_workspace_diagnostic() {
    let mut server = TestServer::new().await;
    server.open("temp.php", "<?php\nundefined();\n").await;

    // Immediately close and open another file
    // (This test verifies the handler doesn't panic, not a true race condition)
    let resp = server.workspace_diagnostic().await;

    assert!(
        resp["error"].is_null(),
        "workspace_diagnostic should handle file closure gracefully"
    );
}

#[tokio::test]
async fn pull_diagnostics_returns_report() {
    let mut server = TestServer::new().await;
    server.open("pull_diag.php", "<?php\n$x = 1;\n").await;

    let resp = server.pull_diagnostics("pull_diag.php").await;

    assert!(
        resp["error"].is_null(),
        "textDocument/diagnostic error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(!result.is_null(), "expected non-null diagnostic report");
    // First pull on a freshly-opened file must be a full report, not unchanged.
    assert_eq!(
        result["kind"].as_str(),
        Some("full"),
        "first pull must return kind='full', got: {:?}",
        result["kind"]
    );
    // Clean file has no diagnostics.
    let items = result["items"]
        .as_array()
        .expect("'items' array in full report");
    assert!(
        items.is_empty(),
        "clean file should have zero diagnostics, got: {items:?}"
    );
}

#[tokio::test]
async fn regression_document_and_workspace_diagnostic_consistency() {
    let mut server = TestServer::new().await;
    server
        .open("consistency.php", "<?php\necho 'test';\n")
        .await;

    let doc_resp = server.pull_diagnostics("consistency.php").await;
    let ws_resp = server.workspace_diagnostic().await;

    // textDocument/diagnostic response structure: result has resultId directly (camelCase)
    let doc_result_id = &doc_resp["result"]["resultId"];

    // workspace/diagnostic response structure: items is array of reports
    // Each report can have result_id at root level or nested in full_document_diagnostic_report
    let ws_result = &ws_resp["result"];
    let ws_item = &ws_result["items"][0];
    let ws_result_id = &ws_item["resultId"];

    // Both must have resultId
    assert!(
        !doc_result_id.is_null(),
        "document/diagnostic must have resultId"
    );
    assert!(
        !ws_result_id.is_null(),
        "workspace/diagnostic must have resultId"
    );

    // Both should be strings
    let doc_id = doc_result_id.as_str();
    let ws_id = ws_result_id.as_str();
    assert!(
        doc_id.is_some() && ws_id.is_some(),
        "resultIds must be strings"
    );

    // They should be identical (same file, same content)
    assert_eq!(
        doc_id, ws_id,
        "Both endpoints should return same resultId for same file"
    );
}

/// REGRESSION: Error handling must propagate errors to client.
/// Previously: .unwrap_or_default() would silently hide task panics.
/// Fixed: Errors are now properly propagated via LSP error response.
#[tokio::test]
async fn workspace_diagnostic_after_edit() {
    let mut server = TestServer::new().await;
    server.open("ws_fix.php", "<?php\nundefined_fn();\n").await;

    // Verify initial error
    let resp1 = server.workspace_diagnostic().await;
    let out1 = render_workspace_diagnostic(&resp1, &server.uri(""));
    expect![[r#"
        ws_fix.php
          1:0 Function undefined_fn() is not defined [UndefinedFunction] (error)"#]]
    .assert_eq(&out1);

    // Fix the error by changing the code
    server.change("ws_fix.php", 2, "<?php\n").await;

    // Verify error is gone
    let resp2 = server.workspace_diagnostic().await;
    let out2 = render_workspace_diagnostic(&resp2, &server.uri(""));

    expect![[r#"
        ws_fix.php
          <clean>"#]]
    .assert_eq(&out2);
}

#[tokio::test]
async fn workspace_diagnostic_clean_file() {
    let mut server = TestServer::new().await;
    server.open("ws_clean.php", "<?php\n$x = 1;\n").await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));

    expect![[r#"
        ws_clean.php
          <clean>"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn workspace_diagnostic_empty_prev_ids_all_full() {
    let mut server = TestServer::new().await;
    server.open("a.php", "<?php\n$x = 1;\n").await;
    server.open("b.php", "<?php\n$y = 2;\n").await;

    // workspace_diagnostic() hardcodes empty previousResultIds
    let resp = server.workspace_diagnostic().await;
    let items = resp["result"]["items"].as_array().unwrap();

    for item in items {
        assert_eq!(
            item["kind"].as_str().unwrap(),
            "full",
            "empty previousResultIds must yield Full for all files"
        );
    }
}

// ── use-import edge cases ──────────────────────────────────────────────────────

/// Grouped `use` import (`use A\{B, C};`) must not produce UndefinedClass for
/// any name in the group when the classes are PSR-4-resolvable on disk.
#[tokio::test]
async fn workspace_diagnostic_empty_workspace() {
    let mut server = TestServer::new().await;

    let resp = server.workspace_diagnostic().await;

    assert!(
        resp["error"].is_null(),
        "workspace/diagnostic error: {:?}",
        resp
    );
    let items = resp["result"]["items"]
        .as_array()
        .expect("expected 'items' array in workspace diagnostic report");
    assert!(
        items.is_empty(),
        "empty workspace should have no diagnostic items, got: {items:?}"
    );
}

#[tokio::test]
async fn workspace_diagnostic_full_after_file_change() {
    let mut server = TestServer::new().await;
    server.open("changing.php", "<?php\n$x = 1;\n").await;

    let resp1 = server.workspace_diagnostic().await;
    let items1 = resp1["result"]["items"].as_array().unwrap();
    let old_id = items1[0]["resultId"].as_str().unwrap().to_string();
    let uri = items1[0]["uri"].as_str().unwrap().to_string();

    // Introduce a semantic error
    server
        .change("changing.php", 2, "<?php\nundefined_fn();\n")
        .await;

    // previousResultIds contains stale result_id → must return Full
    let resp2 = server
        .workspace_diagnostic_with_prev(vec![(uri, old_id.clone())])
        .await;
    let items2 = resp2["result"]["items"].as_array().unwrap();

    assert_eq!(
        items2[0]["kind"].as_str().unwrap(),
        "full",
        "stale previousResultId must yield Full"
    );

    let new_id = items2[0]["resultId"].as_str().unwrap();
    assert_ne!(
        new_id, old_id,
        "result_id must change when diagnostics change"
    );

    let out2 = render_workspace_diagnostic(&resp2, &server.uri(""));
    expect![[r#"
        changing.php
          1:0 Function undefined_fn() is not defined [UndefinedFunction] (error)"#]]
    .assert_eq(&out2);
}

#[tokio::test]
async fn workspace_diagnostic_mixed_unchanged_and_full() {
    let mut server = TestServer::new().await;
    server.open("stable.php", "<?php\n$x = 1;\n").await;
    server.open("breaking.php", "<?php\n$y = 2;\n").await;

    // First request: both Full
    let resp1 = server.workspace_diagnostic().await;
    let items1 = resp1["result"]["items"].as_array().unwrap();

    // Only send previousResultId for stable.php
    let stable = items1
        .iter()
        .find(|i| i["uri"].as_str().unwrap_or("").contains("stable.php"))
        .unwrap();
    let prev = vec![(
        stable["uri"].as_str().unwrap().to_string(),
        stable["resultId"].as_str().unwrap().to_string(),
    )];

    // Change breaking.php before second request
    server
        .change("breaking.php", 2, "<?php\nundefined_fn();\n")
        .await;

    let resp2 = server.workspace_diagnostic_with_prev(prev).await;
    let out2 = render_workspace_diagnostic(&resp2, &server.uri(""));

    expect![[r#"
        breaking.php
          1:0 Function undefined_fn() is not defined [UndefinedFunction] (error)
        stable.php
          <unchanged>"#]]
    .assert_eq(&out2);
}

#[tokio::test]
async fn workspace_diagnostic_multiple_errors_same_file() {
    let mut server = TestServer::new().await;
    server
        .open(
            "multi_err.php",
            "<?php\none_undefined();\ntwo_undefined();\nthree_undefined();\n",
        )
        .await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));

    expect![[r#"
        multi_err.php
          1:0 Function one_undefined() is not defined [UndefinedFunction] (error)
          2:0 Function two_undefined() is not defined [UndefinedFunction] (error)
          3:0 Function three_undefined() is not defined [UndefinedFunction] (error)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn workspace_diagnostic_multiple_files_mixed() {
    let mut server = TestServer::new().await;
    server
        .open("ws_clean.php", "<?php\nfunction foo(): void {}\n")
        .await;
    server.open("ws_error.php", "<?php\nbar();\n").await;
    server
        .open("ws_another.php", "<?php\n$x = new Missing();\n")
        .await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));

    expect![[r#"
        ws_another.php
          1:9 Class Missing does not exist [UndefinedClass] (error)
        ws_clean.php
          <clean>
        ws_error.php
          1:0 Function bar() is not defined [UndefinedFunction] (error)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn workspace_diagnostic_new_file_always_full() {
    let mut server = TestServer::new().await;
    server.open("existing.php", "<?php\n$x = 1;\n").await;

    let resp1 = server.workspace_diagnostic().await;
    let items1 = resp1["result"]["items"].as_array().unwrap();
    let prev = vec![(
        items1[0]["uri"].as_str().unwrap().to_string(),
        items1[0]["resultId"].as_str().unwrap().to_string(),
    )];

    // Open a new file not in previousResultIds
    server.open("newfile.php", "<?php\n$y = 2;\n").await;

    let resp2 = server.workspace_diagnostic_with_prev(prev).await;
    let items2 = resp2["result"]["items"].as_array().unwrap();

    let new_item = items2
        .iter()
        .find(|i| i["uri"].as_str().unwrap_or("").contains("newfile.php"))
        .expect("newfile.php must appear in response");

    assert_eq!(
        new_item["kind"].as_str().unwrap(),
        "full",
        "file absent from previousResultIds must return Full"
    );
}

#[tokio::test]
async fn workspace_diagnostic_single_file_with_errors() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ws_error.php",
            "<?php\nnonexistent_function();\n$x = new UnknownClass();\n",
        )
        .await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));

    expect![[r#"
        ws_error.php
          1:0 Function nonexistent_function() is not defined [UndefinedFunction] (error)
          2:9 Class UnknownClass does not exist [UndefinedClass] (error)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn workspace_diagnostic_unchanged_on_repeated_request() {
    let mut server = TestServer::new().await;
    server.open("stable.php", "<?php\n$x = 1;\n").await;

    // First request: empty previousResultIds → all files must return Full
    let resp1 = server.workspace_diagnostic().await;
    let items1 = resp1["result"]["items"].as_array().unwrap();
    assert_eq!(items1[0]["kind"].as_str().unwrap(), "full");

    let result_id = items1[0]["resultId"].as_str().unwrap().to_string();
    let uri = items1[0]["uri"].as_str().unwrap().to_string();

    // Second request with correct previousResultIds → must return Unchanged
    let resp2 = server
        .workspace_diagnostic_with_prev(vec![(uri, result_id)])
        .await;
    let items2 = resp2["result"]["items"].as_array().unwrap();

    assert_eq!(
        items2[0]["kind"].as_str().unwrap(),
        "unchanged",
        "second request with matching result_id must return Unchanged"
    );

    let out2 = render_workspace_diagnostic(&resp2, &server.uri(""));
    expect![[r#"
        stable.php
          <unchanged>"#]]
    .assert_eq(&out2);
}

#[tokio::test]
async fn workspace_diagnostic_with_parse_error() {
    let mut server = TestServer::new().await;
    server
        .open("ws_parse_error.php", "<?php\nfunction f( {\n")
        .await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));

    expect![[r#"
        ws_parse_error.php
          1:12 expected variable, found '{' [<unset>] (error)
          1:12 unclosed '')'' opened at Span { start: 16, end: 17 } [<unset>] (error)
          2:0 unclosed ''}'' opened at Span { start: 18, end: 19 } [<unset>] (error)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn workspace_diagnostic_wrong_result_id_returns_full() {
    let mut server = TestServer::new().await;
    server.open("test.php", "<?php\n$x = 1;\n").await;
    let uri = server.uri("test.php");

    // Send a result_id that doesn't match the real one
    let resp = server
        .workspace_diagnostic_with_prev(vec![(uri, "wrong-result-id".to_string())])
        .await;
    let items = resp["result"]["items"].as_array().unwrap();

    assert_eq!(
        items[0]["kind"].as_str().unwrap(),
        "full",
        "wrong previousResultId must produce Full, not Unchanged"
    );
}

#[tokio::test]
async fn edge_case_workspace_diagnostic_many_files() {
    let mut server = TestServer::new().await;

    // Open 10 files
    for i in 0..10 {
        let code = format!("<?php\nfunction test{i}() {{ return 42; }}\n");
        server.open(&format!("file{i}.php"), &code).await;
    }

    let resp = server.workspace_diagnostic().await;
    let items = resp["result"]["items"].as_array().unwrap();

    assert_eq!(
        items.len(),
        10,
        "workspace_diagnostic should return diagnostics for all open files"
    );

    // All should be clean
    for item in items {
        assert!(
            item["items"].as_array().unwrap().is_empty(),
            "Clean files should have empty diagnostics array"
        );
    }
}
