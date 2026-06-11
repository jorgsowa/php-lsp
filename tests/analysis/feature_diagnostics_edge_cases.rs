//! Diagnostic coverage matrix using the caret annotation DSL.
//! Each test names the expectation inline with `// ^^^ severity: message`.

use super::*;

use expect_test::expect;
use serde_json::json;

#[tokio::test]
async fn builtin_restore_error_handler_is_known() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function _wrap(): void {
    restore_error_handler();
}
"#,
    )
    .await;
}

/// Reproducer: a project polyfill that conditionally redefines a built-in.
/// If `ingest_stub_slice` is last-write-wins and the project file's parsed
/// `function restore_error_handler` overrides mir's stub, the call site may
/// still resolve — but the polyfill body is what ends up authoritative. This
/// test asserts that the call is *not* flagged undefined when a user-land
/// polyfill exists in the workspace.
#[tokio::test]
async fn clean_file_has_no_diagnostics() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function f(string $x): string { return $x; }
f('ok');
"#,
    )
    .await;
}

#[tokio::test]
async fn diagnostics_clear_after_fix() {
    let mut s = TestServer::new().await;
    let notif = s.open("fix.php", "<?php\nundefined_fn();\n").await;
    assert!(
        !notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty()
    );
    let after = s.change("fix.php", 2, "<?php\n").await;
    assert!(
        after["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn did_open_reports_deprecated_call_warning() {
    let mut server = TestServer::new().await;
    let notif = server
        .open(
            "deprecated_test.php",
            "<?php\n/** @deprecated Use newFunc() instead */\nfunction oldFunc(): void {}\n\noldFunc();\n",
        )
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap();
    let hit = diags.iter().find(|d| {
        d["code"].as_str() == Some("DeprecatedCall")
            && d["message"]
                .as_str()
                .map(|m| m.contains("oldFunc"))
                .unwrap_or(false)
    });
    assert!(
        hit.is_some(),
        "expected DeprecatedCall diagnostic for oldFunc on did_open, got: {diags:?}"
    );
}

#[tokio::test]
async fn issue_170_errors_inside_namespaced_method_detected() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
namespace LspTest;

class Broken
{
    public int $count = 0;

    public function bump(): int
    {
        $this->count++;
        return $this->count;
    }

    public function obviouslyBroken(): int
    {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
        $x = new UnknownClass();
//               ^^^^^^^^^^^^ error: UnknownClass
        return 0;
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn multiple_diagnostics_same_file() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function _wrap(): void {
    one_undefined();
//  ^^^^^^^^^^^^^^^ error: one_undefined
    two_undefined();
//  ^^^^^^^^^^^^^^^ error: two_undefined
}
"#,
    )
    .await;
}

#[tokio::test]
async fn parse_error_emits_diagnostic() {
    let mut s = TestServer::new().await;
    let notif = s.open("bad.php", "<?php\nfunction f( {\n").await;
    assert!(
        !notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected parse diagnostic for malformed PHP"
    );
}

#[tokio::test]
async fn regression_error_handling() {
    let mut server = TestServer::new().await;
    server.open("test.php", "<?php\n").await;

    let resp = server.workspace_diagnostic().await;

    // This should always succeed (no parse/semantic errors in clean file)
    assert!(
        resp["error"].is_null(),
        "workspace_diagnostic request should not error for valid files"
    );

    // Check that response structure is valid
    assert!(
        resp["result"]["items"].is_array(),
        "Response should contain items array"
    );
}

/// REGRESSION: result_id must be stable across consecutive requests.
/// Same file with same diagnostics should return same result_id.
#[tokio::test]
async fn regression_params_structure_accepted() {
    let mut server = TestServer::new().await;
    server.open("param_test.php", "<?php\necho 'test';\n").await;

    // Request workspace/diagnostic (which accepts WorkspaceDiagnosticParams)
    let resp = server.workspace_diagnostic().await;

    // Should not error even though params include previousResultIds capability
    assert!(
        resp["error"].is_null(),
        "workspace_diagnostic must accept params without error"
    );

    // Should return valid response structure
    assert!(
        resp["result"]["items"].is_array(),
        "Should return items array"
    );
}

/// CRITICAL: result_id must change when diagnostic properties change.
/// Even if position and message are identical, severity changes must produce different result_id.
/// This was missing from initial hash implementation.
#[tokio::test]
async fn regression_parse_error_files_included() {
    let mut server = TestServer::new().await;
    server
        .open("parse_only.php", "<?php\nfunction broken( {\n")
        .await;

    let resp = server.workspace_diagnostic().await;
    let items = resp["result"]["items"].as_array().unwrap();

    // Parse error files must be included
    assert!(
        !items.is_empty(),
        "Parse error files must appear in workspace/diagnostic"
    );

    // Should have the parse error in diagnostics
    assert!(
        items[0]["items"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "File should have diagnostics (parse error)"
    );
}

/// REGRESSION: result_id must be unique per file for caching.
/// Previously: result_id was always None for all files.
/// Fixed: Each file now gets a deterministic result_id based on content hash.
#[tokio::test]
async fn regression_result_id_changes_with_diagnostics() {
    let mut server = TestServer::new().await;
    server.open("changetest.php", "<?php\n$x = 1;\n").await;

    // Get result_id for clean file
    let resp1 = server.workspace_diagnostic().await;
    let items1 = resp1["result"]["items"].as_array().unwrap();
    let id_clean = items1[0]["resultId"].as_str().unwrap().to_string();

    // Add an error to the file
    server
        .change("changetest.php", 2, "<?php\nundefined_function();\n")
        .await;

    // Get result_id for file with error
    let resp2 = server.workspace_diagnostic().await;
    let items2 = resp2["result"]["items"].as_array().unwrap();
    let id_with_error = items2[0]["resultId"].as_str().unwrap().to_string();

    // result_id must change when diagnostics change
    assert_ne!(
        id_clean, id_with_error,
        "result_id must change when diagnostics change"
    );

    // Verify the error is actually there
    assert!(
        !items2[0]["items"].as_array().unwrap().is_empty(),
        "File should have diagnostics after adding error"
    );

    // Fix the error
    server.change("changetest.php", 2, "<?php\n$x = 1;\n").await;

    // Get result_id for fixed file
    let resp3 = server.workspace_diagnostic().await;
    let items3 = resp3["result"]["items"].as_array().unwrap();
    let id_fixed = items3[0]["resultId"].as_str().unwrap().to_string();

    // Should revert to original result_id
    assert_eq!(
        id_clean, id_fixed,
        "result_id should revert when diagnostics return to original state"
    );
}

/// REGRESSION: document/diagnostic and workspace/diagnostic must both use result_id.
/// Previously: Both handlers set result_id to None.
/// Fixed: Both now generate consistent, deterministic result_ids.
#[tokio::test]
async fn regression_result_id_is_present() {
    let mut server = TestServer::new().await;
    server.open("test1.php", "<?php\n$x = 1;\n").await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));
    expect![[r#"
        test1.php
          <clean>"#]]
    .assert_eq(&out);
    let items = resp["result"]["items"].as_array().unwrap();
    let result_id = &items[0]["resultId"];
    assert!(
        !result_id.is_null(),
        "REGRESSION: resultId must be non-null. \
         Clients need this to implement caching via previousResultIds."
    );

    // Verify it's a string, not some other JSON type
    assert!(
        result_id.is_string(),
        "resultId should be a string (format: v1:hash)"
    );
}

/// REGRESSION: Files with parse errors must appear in workspace/diagnostic.
/// Previously: There was potential for parse-error-only files to be filtered out.
/// This test verifies parse errors are correctly included.
#[tokio::test]
async fn regression_result_id_is_stable() {
    let mut server = TestServer::new().await;
    server.open("stable.php", "<?php\necho 'hello';\n").await;

    // First request
    let resp1 = server.workspace_diagnostic().await;
    let items1 = resp1["result"]["items"].as_array().unwrap();
    let id1 = items1[0]["resultId"].as_str().unwrap().to_string();

    // Second request (no changes)
    let resp2 = server.workspace_diagnostic().await;
    let items2 = resp2["result"]["items"].as_array().unwrap();
    let id2 = items2[0]["resultId"].as_str().unwrap().to_string();

    // result_id must be identical (deterministic hash)
    assert_eq!(
        id1, id2,
        "result_id must be stable for unchanged file (deterministic hashing)"
    );
}

/// REGRESSION: result_id must account for all diagnostic types.
/// File with both parse errors and semantic errors should have result_id that reflects both.
#[tokio::test]
async fn regression_result_id_reflects_all_diagnostic_properties() {
    let mut server = TestServer::new().await;

    // Open file with undefined function (error severity)
    server
        .open(
            "props1.php",
            "<?php\nfunction test() {}\nundefined_func();\n",
        )
        .await;

    let resp1 = server.workspace_diagnostic().await;
    let out1 = render_workspace_diagnostic(&resp1, &server.uri(""));
    expect![[r#"
        props1.php
          2:0 Function undefined_func() is not defined [UndefinedFunction] (error)"#]]
    .assert_eq(&out1);
    let items1 = resp1["result"]["items"].as_array().unwrap();
    let result_id_1 = items1[0]["resultId"].as_str().unwrap().to_string();

    // Open different file with undefined variable (different code/severity)
    server
        .open("props2.php", "<?php\necho $undefined_var;\n")
        .await;

    let resp2 = server.workspace_diagnostic().await;
    let items2 = resp2["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["uri"]
                .as_str()
                .map(|uri| uri.contains("props2.php"))
                .unwrap_or(false)
        })
        .unwrap();

    let result_id_2 = items2["resultId"].as_str().unwrap();

    // Different diagnostic codes/types should produce different result_ids
    // (UndefinedFunction vs UndefinedVariable)
    assert_ne!(
        result_id_1, result_id_2,
        "Different diagnostic codes should produce different result_ids \
         (even if both are 1 error). Hash must include code field."
    );
}

// ─────────────────────────────────────────────────────────────────────────
// EDGE CASE TESTS - Stress scenarios and boundary conditions
// ─────────────────────────────────────────────────────────────────────────

/// EDGE CASE: Very large workspace with many files.
/// workspace_diagnostic iterates all open files and runs semantic analysis on each.
/// Should verify it doesn't have quadratic behavior or memory issues.
#[tokio::test]
async fn regression_result_id_unique_per_file() {
    let mut server = TestServer::new().await;
    server.open("file1.php", "<?php\necho 'a';\n").await;
    server.open("file2.php", "<?php\necho 'b';\n").await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));
    expect![[r#"
        file1.php
          <clean>
        file2.php
          <clean>"#]]
    .assert_eq(&out);
    let items = resp["result"]["items"].as_array().unwrap();
    let id1 = items[0]["resultId"].as_str().unwrap();
    let id2 = items[1]["resultId"].as_str().unwrap();

    // Different files should have different result_ids (different content)
    assert_ne!(
        id1, id2,
        "Different files with different content should have different result_ids"
    );
}

/// REGRESSION: result_id must change when diagnostics change.
/// Previously: result_id was always None.
/// Fixed: result_id is now based on diagnostic content, so it changes when errors appear/disappear.
#[tokio::test]
async fn regression_result_id_with_mixed_diagnostics() {
    let mut server = TestServer::new().await;

    // File with semantic error (no parse error)
    server
        .open(
            "semantic.php",
            "<?php\nfunction foo() {}\nundefined_func();\n",
        )
        .await;

    let resp1 = server.workspace_diagnostic().await;
    let items1 = resp1["result"]["items"].as_array().unwrap();
    let id_semantic = items1[0]["resultId"].as_str().unwrap();

    // Different file with only parse error
    server
        .open("parse.php", "<?php\nfunction broken( {\n")
        .await;

    let resp2 = server.workspace_diagnostic().await;
    let items2 = resp2["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["uri"]
                .as_str()
                .map(|uri| uri.contains("parse.php"))
                .unwrap_or(false)
        })
        .unwrap();
    let id_parse = items2["resultId"].as_str().unwrap();

    // Different error types should produce different result_ids
    assert_ne!(
        id_semantic, id_parse,
        "result_id should differ for different diagnostic types"
    );
}

/// REGRESSION: workspace_diagnostic must accept params without error.
/// The LSP spec allows clients to send previousResultIds in params.
/// Handler must accept params structure gracefully (even if not using Unchanged variant yet).
#[tokio::test]
async fn requests_on_parse_error_file_do_not_error() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("broken.php", "<?php\nfunction f( $x { // missing ): body\n")
        .await;

    let diags = notif["params"]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !diags.is_empty(),
        "expected parse diagnostics for broken source"
    );

    let resp = server.hover("broken.php", 1, 10).await;
    assert!(resp["error"].is_null(), "hover errored: {resp:?}");

    let resp = server.document_symbols("broken.php").await;
    assert!(resp["error"].is_null(), "documentSymbol errored: {resp:?}");

    let resp = server.folding_range("broken.php").await;
    assert!(resp["error"].is_null(), "foldingRange errored: {resp:?}");
}

#[tokio::test]
async fn same_namespace_truly_missing_class_is_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    // No `Missing` class exists anywhere on disk.
    let consumer_src = "<?php\nnamespace App;\nclass Consumer {\n    public function __construct(private Missing $m) {}\n}\n";
    std::fs::write(tmp.path().join("src/Consumer.php"), consumer_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Consumer.php", consumer_src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    assert!(
        out.contains("UndefinedClass") && out.contains("App\\Missing"),
        "expected UndefinedClass for App\\Missing, got:\n{out}"
    );
}

#[tokio::test]
async fn user_polyfill_does_not_break_builtin_restore_error_handler() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"//- /src/polyfill.php
<?php
if (!function_exists('restore_error_handler')) {
    function restore_error_handler(): bool { return true; }
}

//- /src/main.php
<?php
function _wrap(): void {
    restore_error_handler();
}
"#,
    )
    .await;
}

/// Reproducer: an unconditional user-land redefinition of a built-in.
/// PHP would refuse this at runtime, but the LSP still parses it; if the
/// stub-ingest path is last-write-wins, the project's body silently replaces
/// mir's stub. The call site should still resolve.
#[tokio::test]
async fn user_unconditional_redefinition_does_not_break_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_diagnostics(
        r#"//- /src/redef.php
<?php
function restore_error_handler(): bool { return true; }

//- /src/main.php
<?php
function _wrap(): void {
    restore_error_handler();
}
"#,
    )
    .await;
}

/// Duplicate class declaration in the same file should produce an error.
/// mir emits DuplicateClass over the whole declaration span (col 0–12), which
/// the `// ^^^` DSL cannot represent (minimum addressable col is 2), so we
/// check the raw notification instead.
#[tokio::test]
async fn duplicate_class_declaration_emits_warning() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
class Foo {}
class Foo {}
"#,
        )
        .await;
    let diags = opened.diagnostics_for("main.php");
    let items = diags["params"]["diagnostics"].as_array().unwrap();
    let dup = items
        .iter()
        .find(|d| {
            d["code"].as_str() == Some("DuplicateClass")
                && d["range"]["start"]["line"].as_u64() == Some(2)
                && d["severity"].as_u64() == Some(1)
        })
        .unwrap_or_else(|| {
            panic!(
                "expected a DuplicateClass error on line 2, got: {:#?}",
                diags["params"]["diagnostics"]
            )
        });
    assert!(
        dup["message"]
            .as_str()
            .unwrap_or("")
            .contains("has already been defined"),
        "unexpected message: {}",
        dup["message"]
    );
}

/// Duplicate interface declaration in the same file should produce a warning.
#[tokio::test]
async fn duplicate_interface_declaration_emits_warning() {
    let mut s = TestServer::new().await;
    // The duplicate is indented so both ranges are expressible as annotations
    // (the `//` prefix occupies columns 0-1). mir ≥0.36 reports its own
    // DuplicateInterface error alongside php-lsp's duplicate-declaration
    // warning.
    s.check_diagnostics(
        r#"<?php
interface Logger {}
  interface Logger {}
//^^^^^^^^^^^^^^^^^^^ error: Interface Logger has already been defined
//          ^^^^^^ warning: Duplicate declaration
"#,
    )
    .await;
}

/// Duplicate trait declaration in the same file should produce a warning.
#[tokio::test]
async fn duplicate_trait_declaration_emits_warning() {
    let mut s = TestServer::new().await;
    // See duplicate_interface_declaration_emits_warning for the indentation
    // rationale; mir ≥0.36 adds its own DuplicateTrait error.
    s.check_diagnostics(
        r#"<?php
trait Serializable {}
  trait Serializable {}
//^^^^^^^^^^^^^^^^^^^^^ error: Trait Serializable has already been defined
//      ^^^^^^^^^^^^ warning: Duplicate declaration
"#,
    )
    .await;
}

/// Classes with the same short name in different namespaces must NOT be flagged.
#[tokio::test]
async fn duplicate_class_different_namespaces_not_flagged() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
namespace AppA;
class Foo {}

namespace AppB;
class Foo {}
"#,
    )
    .await;
}

/// `abs(int)` returns `int` when the argument is an `int` literal or parameter.
/// The mir analyzer currently reports `float|int` for the return type, causing a
/// false-positive type-mismatch when the result is passed to an `int` parameter.
/// Tracked in the mir analyzer; this test documents the expected clean state.
#[tokio::test]
async fn abs_of_int_arg_not_flagged_as_float() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
function takesInt(int $x): void {}
function test(int $n): void {
    takesInt(abs($n));
}
"#,
    )
    .await;
}
