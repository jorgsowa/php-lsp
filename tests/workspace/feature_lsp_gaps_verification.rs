/// Comprehensive verification of LSP feature gaps and edge cases.
use super::*;
use expect_test::expect;
use serde_json::{Value, json};

// ============================================================================
// PULL DIAGNOSTICS EDGE CASES
// ============================================================================

#[tokio::test]
async fn pull_diagnostics_multi_parse_errors_all_returned() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
function foo( {
class {
const X
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

    expect![[r#"
        1:14-1:15 [1] SyntaxError: expected variable, found '{'
        1:14-1:15 [1] SyntaxError: unclosed '')'' opened at 1:12
        2:6-2:7 [1] SyntaxError: expected class name, found '{'
        4:0-4:1 [1] SyntaxError: expected ';', found end of file
        4:0-4:1 [1] SyntaxError: expected '=', found end of file
        4:0-4:1 [1] SyntaxError: expected '}', found end of file
        4:0-4:1 [1] SyntaxError: expected constant name, found end of file
        4:0-4:1 [1] SyntaxError: expected expression
        4:0-4:1 [1] SyntaxError: unclosed ''}'' opened at 1:14"#]]
    .assert_eq(&render_pull_diagnostics(&resp));
}

#[tokio::test]
async fn pull_diagnostics_on_nonexistent_file_returns_empty() {
    let mut s = TestServer::new().await;
    let uri = s.uri("nonexistent.php");

    // Don't open the file - request diagnostics directly
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
async fn pull_diagnostics_mixed_parse_and_semantic_errors() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
class Foo {
    public function bar(

    public function undefined_call() {
        nonexistent_func();
    }
}"#,
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

    expect![[r#"
        4:11-4:19 [1] SyntaxError: expected ')', found 'function'
        4:11-4:19 [1] SyntaxError: expected ';', found 'function'
        4:11-4:19 [1] SyntaxError: expected variable, found 'function'
        4:4-4:10 [1] SyntaxError: Cannot declare promoted property outside a constructor
        5:8-5:26 [1] UndefinedFunction: Function nonexistent_func() is not defined"#]]
    .assert_eq(&render_pull_diagnostics(&resp));
}

#[tokio::test]
async fn pull_diagnostics_incremental_changes_update_result() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open("test.php", "<?php\n$x = 1;").await;

    let resp1 = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()}
            }),
        )
        .await;

    let id1 = resp1["result"]["resultId"].clone();

    // Make a change that introduces an error
    s.client()
        .notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": uri.to_string(), "version": 2},
                "contentChanges": [{
                    "text": "<?php\nundefined_function();"
                }]
            }),
        )
        .await;

    // Request new diagnostics
    let resp2 = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri.to_string()}
            }),
        )
        .await;

    let id2 = resp2["result"]["resultId"].clone();

    // Result ID should change when content changes
    assert_ne!(
        id1, id2,
        "result_id should change after content modification"
    );

    expect!["<empty>"].assert_eq(&render_pull_diagnostics(&resp1));
    expect!["1:0-1:20 [1] UndefinedFunction: Function undefined_function() is not defined"]
        .assert_eq(&render_pull_diagnostics(&resp2));
}

#[tokio::test]
async fn pull_diagnostics_with_namespace_code() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
namespace App\Services;

class UserService {
    public function create(string $name): User {
        return new User($name);
    }
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

    expect![[r#"
        4:42-4:46 [1] UndefinedClass: Class App\Services\User does not exist
        5:19-5:23 [1] UndefinedClass: Class App\Services\User does not exist"#]]
    .assert_eq(&render_pull_diagnostics(&resp));
}

// ============================================================================
// WORKSPACE/APPLYWORLDEDIT VERIFICATION
// ============================================================================

#[tokio::test]
async fn apply_edit_advertised_in_capabilities() {
    // The capability should be advertised - verify via initialize response
    let (_, init_resp) = TestServer::new_with_options(json!({})).await;

    let workspace = &init_resp["result"]["capabilities"]["workspace"];
    expect![[r#"{"fileOperations":{"didCreate":{"filters":[{"pattern":{"glob":"**/*.php","matches":"file"},"scheme":"file"}]},"didDelete":{"filters":[{"pattern":{"glob":"**/*.php","matches":"file"},"scheme":"file"}]},"didRename":{"filters":[{"pattern":{"glob":"**/*.php","matches":"file"},"scheme":"file"}]},"willCreate":{"filters":[{"pattern":{"glob":"**/*.php","matches":"file"},"scheme":"file"}]},"willDelete":{"filters":[{"pattern":{"glob":"**/*.php","matches":"file"},"scheme":"file"}]},"willRename":{"filters":[{"pattern":{"glob":"**/*.php","matches":"file"},"scheme":"file"}]}},"workspaceFolders":{"changeNotifications":true,"supported":true}}"#]].assert_eq(&workspace.to_string());
}

// ============================================================================
// CODE ACTION KINDS VERIFICATION
// ============================================================================

#[tokio::test]
async fn code_action_kinds_advertised_in_capabilities() {
    let (_, init_resp) = TestServer::new_with_options(json!({})).await;
    let kinds = &init_resp["result"]["capabilities"]["codeActionProvider"]["codeActionKinds"];
    expect![[
        r#"["quickfix","refactor","refactor.extract","refactor.inline","source.organizeImports"]"#
    ]]
    .assert_eq(&kinds.to_string());
}

// ============================================================================
// DIAGNOSTIC PROVIDER VERIFICATION
// ============================================================================

#[tokio::test]
async fn diagnostic_provider_advertised_in_capabilities() {
    let (_, init_resp) = TestServer::new_with_options(json!({})).await;
    let diag_provider = &init_resp["result"]["capabilities"]["diagnosticProvider"];
    expect![[r#"{"interFileDependencies":true,"workspaceDiagnostics":true}"#]]
        .assert_eq(&diag_provider.to_string());
}

#[tokio::test]
async fn pull_diagnostics_sequential_files() {
    let mut s = TestServer::new().await;
    let uri1 = s.uri("test1.php");
    let uri2 = s.uri("test2.php");

    s.open("test1.php", "<?php\nclass Foo { }").await;
    s.open("test2.php", "<?php\nfunction bar() {}").await;

    // Make sequential diagnostic requests (not concurrent - avoid borrow issues)
    let resp1 = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri1.to_string()}
            }),
        )
        .await;

    let resp2 = s
        .client()
        .request(
            "textDocument/diagnostic",
            json!({
                "textDocument": {"uri": uri2.to_string()}
            }),
        )
        .await;

    // Results should be different (different files, different result IDs)
    let id1 = resp1["result"]["resultId"].clone();
    let id2 = resp2["result"]["resultId"].clone();
    assert_ne!(id1, id2, "different files should have different result IDs");

    expect!["<empty>"].assert_eq(&render_pull_diagnostics(&resp1));
    expect!["<empty>"].assert_eq(&render_pull_diagnostics(&resp2));
}

#[tokio::test]
async fn pull_diagnostics_with_severity_levels() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
// This will cause an error
undefined_function();
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

    expect!["2:0-2:20 [1] UndefinedFunction: Function undefined_function() is not defined"]
        .assert_eq(&render_pull_diagnostics(&resp));
}

#[tokio::test]
async fn pull_diagnostics_range_precision() {
    let mut s = TestServer::new().await;
    let uri = s.uri("test.php");

    s.open(
        "test.php",
        r#"<?php
$undefined_var = $x + 1;
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

    expect!["1:17-1:19 [1] UndefinedVariable: Variable $x is not defined"]
        .assert_eq(&render_pull_diagnostics(&resp));
}

#[tokio::test]
async fn pull_diagnostics_source_field_present() {
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

    let items = resp["result"]["items"].as_array().unwrap();

    // All diagnostics should have a source field identifying them as from php-lsp
    for item in items {
        assert!(
            item.get("source").is_some(),
            "diagnostic should have source field"
        );
        let source = item["source"].as_str().unwrap();
        assert_eq!(
            source, "php-lsp",
            "source should be 'php-lsp', got: {}",
            source
        );
    }
}

// ============================================================================
// TYPE HIERARCHY DYNAMIC REGISTRATION GATING
// ============================================================================
//
// `lsp-types` 0.94 has no static `typeHierarchyProvider` capability field, so
// support is advertised only via a `client/registerCapability` call in
// `initialized`. That call is only meaningful to a client that declared
// `textDocument.typeHierarchy.dynamicRegistration` — these tests pin that the
// server checks the capability instead of registering unconditionally.

fn registered_ids(params: &Value) -> Vec<&str> {
    params["registrations"]
        .as_array()
        .expect("registerCapability params should carry a registrations array")
        .iter()
        .map(|r| r["id"].as_str().expect("registration id"))
        .collect()
}

#[tokio::test]
async fn type_hierarchy_registered_when_client_declares_dynamic_registration() {
    let (mut s, _) = TestServer::new_with_client_capabilities(json!({
        "textDocument": {
            "typeHierarchy": { "dynamicRegistration": true }
        }
    }))
    .await;

    let (_, params) = s
        .client()
        .expect_server_request("client/registerCapability")
        .await;
    let ids = registered_ids(&params);
    assert!(
        ids.contains(&"php-lsp-type-hierarchy"),
        "expected php-lsp-type-hierarchy registration, got {ids:?}"
    );
}

#[tokio::test]
async fn type_hierarchy_not_registered_without_client_capability() {
    // No `textDocument.typeHierarchy` capability at all — the common case for
    // a client that never mentions it.
    let (mut s, _) = TestServer::new_with_client_capabilities(json!({})).await;

    let (_, params) = s
        .client()
        .expect_server_request("client/registerCapability")
        .await;
    let ids = registered_ids(&params);
    assert!(
        !ids.contains(&"php-lsp-type-hierarchy"),
        "did not expect php-lsp-type-hierarchy registration, got {ids:?}"
    );
}

#[tokio::test]
async fn type_hierarchy_not_registered_when_dynamic_registration_false() {
    let (mut s, _) = TestServer::new_with_client_capabilities(json!({
        "textDocument": {
            "typeHierarchy": { "dynamicRegistration": false }
        }
    }))
    .await;

    let (_, params) = s
        .client()
        .expect_server_request("client/registerCapability")
        .await;
    let ids = registered_ids(&params);
    assert!(
        !ids.contains(&"php-lsp-type-hierarchy"),
        "did not expect php-lsp-type-hierarchy registration, got {ids:?}"
    );
}

#[tokio::test]
async fn pull_diagnostics_message_field_present() {
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
        2:0-2:1 [1] SyntaxError: expected '}', found end of file"#]]
    .assert_eq(&out);
}
