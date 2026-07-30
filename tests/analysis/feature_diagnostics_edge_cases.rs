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
    expect!["1:0-1:14 [1] UndefinedFunction: Function undefined_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
    let after = s.change("fix.php", 2, "<?php\n").await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&after));
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
    expect![
        "4:0-4:9 [3] DeprecatedCall: Call to deprecated function oldFunc: Use newFunc() instead"
    ]
    .assert_eq(&render_diagnostics_notification(&notif));
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
    expect![[r#"
        1:12-1:13 [1] SyntaxError: expected variable, found '{'
        1:12-1:13 [1] SyntaxError: unclosed '')'' opened at 1:10
        2:0-2:1 [1] SyntaxError: unclosed ''}'' opened at 1:12"#]]
    .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn regression_error_handling() {
    let mut server = TestServer::new().await;
    server.open("test.php", "<?php\n").await;

    let resp = server.workspace_diagnostic().await;

    expect![[r#"
        test.php
          <clean>"#]]
    .assert_eq(&render_workspace_diagnostic(&resp, &server.uri("")));
}

/// REGRESSION: workspace/diagnostic must accept the `previousResultIds`
/// field in its params without erroring, even before the server uses it to
/// return an `Unchanged` variant.
#[tokio::test]
async fn regression_params_structure_accepted() {
    let mut server = TestServer::new().await;
    server.open("param_test.php", "<?php\necho 'test';\n").await;

    // Request workspace/diagnostic (which accepts WorkspaceDiagnosticParams)
    let resp = server.workspace_diagnostic().await;

    expect![[r#"
        param_test.php
          <clean>"#]]
    .assert_eq(&render_workspace_diagnostic(&resp, &server.uri("")));
}

/// REGRESSION: Files with parse errors must appear in workspace/diagnostic.
/// Previously: there was potential for parse-error-only files to be filtered out.
/// This test verifies parse errors are correctly included.
#[tokio::test]
async fn regression_parse_error_files_included() {
    let mut server = TestServer::new().await;
    server
        .open("parse_only.php", "<?php\nfunction broken( {\n")
        .await;

    let resp = server.workspace_diagnostic().await;
    expect![[r#"
        parse_only.php
          1:17 expected variable, found '{' [SyntaxError] (error)
          1:17 unclosed '')'' opened at 1:15 [SyntaxError] (error)
          2:0 unclosed ''}'' opened at 1:17 [SyntaxError] (error)"#]]
    .assert_eq(&render_workspace_diagnostic(&resp, &server.uri("")));
}

/// REGRESSION: result_id must change when diagnostics change.
/// Previously: result_id was always None.
/// Fixed: result_id is now based on diagnostic content, so it changes when
/// errors appear/disappear, and reverts when the fix is undone.
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

    // Verify the error is actually there — snapshot pins what the error is.
    let resp2_rendered = render_workspace_diagnostic(&resp2, &server.uri(""));
    assert!(
        !resp2_rendered.contains("<clean>"),
        "changetest.php should have diagnostics after adding error: {resp2_rendered}"
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

/// REGRESSION: resultId must be present and a non-null string on every
/// workspace/diagnostic item — clients need it to implement caching via
/// `previousResultIds`.
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

/// REGRESSION: result_id must be stable across consecutive requests.
/// Same file with same diagnostics should return the same result_id.
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

/// REGRESSION: result_id must account for all diagnostic properties, not
/// just error count. Two files each with exactly one error, but of a
/// different diagnostic code, must hash to different result_ids.
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

/// REGRESSION: result_id must be unique per file for caching.
/// Previously: result_id was always None for all files.
/// Fixed: Each file now gets a deterministic result_id based on content hash.
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
    expect![[r#"
        1:15-1:16 [1] SyntaxError: unclosed '')'' opened at 1:10
        2:0-2:1 [1] SyntaxError: unclosed ''}'' opened at 1:15"#]]
    .assert_eq(&render_diagnostics_notification(&notif));

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

    // Whether `Missing` is truly undefined or just not indexed yet is
    // indistinguishable until the scan finishes, so wait for it — see
    // `compute_open_file_diagnostics` and issue #242.
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("src/Consumer.php", consumer_src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Consumer.php
          3:40 Class App\Missing does not exist [UndefinedClass] (error)"#]]
    .assert_eq(&out);
}

/// Reproducer: a project polyfill that conditionally redefines a built-in.
/// If `ingest_stub_slice` is last-write-wins and the project file's parsed
/// `function restore_error_handler` overrides mir's stub, the call site may
/// still resolve — but the polyfill body is what ends up authoritative. This
/// test asserts that the call is *not* flagged undefined when a user-land
/// polyfill exists in the workspace.
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
    assert_eq!(
        dup["message"].as_str().unwrap_or(""),
        "Class Foo has already been defined",
        "unexpected duplicate-class message"
    );
}

/// Duplicate interface declaration in the same file should produce an error.
#[tokio::test]
async fn duplicate_interface_declaration_emits_warning() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
interface Logger {}
  interface Logger {}
//^^^^^^^^^^^^^^^^^^^ error: Interface Logger has already been defined
"#,
    )
    .await;
}

/// Duplicate trait declaration in the same file should produce an error.
#[tokio::test]
async fn duplicate_trait_declaration_emits_warning() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
trait Serializable {}
  trait Serializable {}
//^^^^^^^^^^^^^^^^^^^^^ error: Trait Serializable has already been defined
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
/// Regression guard for a past false-positive where the analyzer reported
/// `float|int` for the return type, flagging a type mismatch when the result
/// was passed to an `int` parameter (fixed upstream in mir; kept green here).
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

// ── diagnostics.missingTypes ──────────────────────────────────────────────────

/// `diagnostics.missingTypes` is off by default — interface methods without
/// return/param annotations are not flagged unless opted in.
#[tokio::test]
async fn missing_types_off_by_default() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
interface Logger {
    public function log($message);
}
"#,
    )
    .await;
}

/// With `diagnostics.missingTypes` on, missing param type annotations on
/// interface methods are reported (return type is provided to isolate the param lint).
#[tokio::test]
async fn missing_types_opt_in_flags_interface_method() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "diagnostics": { "missingTypes": true }
    }))
    .await;
    s.check_diagnostics(
        r#"<?php
interface Logger {
    public function log($message): void;
//                      ^^^^^^^^ info: Parameter $message of Logger::log() has no type annotation
}
"#,
    )
    .await;
}

// ── diagnostics.mixedUsage ────────────────────────────────────────────────────

/// `diagnostics.mixedUsage` is off by default — passing `mixed` to a typed
/// parameter produces no diagnostic unless opted in.
#[tokio::test]
async fn mixed_usage_off_by_default() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
function takesString(string $s): void {}
function test(mixed $v): void {
    takesString($v);
}
"#,
    )
    .await;
}

/// With `diagnostics.mixedUsage` on, passing `mixed` to a typed parameter
/// emits a MixedArgument info diagnostic.
#[tokio::test]
async fn mixed_usage_opt_in_flags_mixed_argument() {
    let (mut s, _) = TestServer::new_with_options(json!({
        "diagnostics": { "mixedUsage": true }
    }))
    .await;
    s.check_diagnostics(
        r#"<?php
function takesString(string $s): void {}
function test(mixed $v): void {
    takesString($v);
//              ^^ info: Argument $s of takesString() is mixed
}
"#,
    )
    .await;
}

#[tokio::test]
async fn syntax_error_produces_error_diagnostic() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let notif = s.open("syntax_err.php", "<?php\nclass {\n").await;
    expect![[r#"
        1:6-1:7 [1] SyntaxError: expected class name, found '{'
        2:0-2:1 [1] SyntaxError: expected '}', found end of file"#]]
    .assert_eq(&render_diagnostics_notification(&notif));
}

// ── nullsafe operator (?->) ───────────────────────────────────────────────────

/// The nullsafe operator explicitly guards against null and must suppress the
/// null-dereference diagnostic a plain `->` on the same value would trigger.
#[tokio::test]
async fn nullsafe_method_call_suppresses_null_diagnostic() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
class Box { public function value(): int { return 1; } }
$box = null;
$box?->value();
"#,
    )
    .await;
}

#[tokio::test]
async fn nullsafe_property_fetch_suppresses_null_diagnostic() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
class Box { public int $value = 0; }
$box = null;
echo $box?->value;
"#,
    )
    .await;
}

#[tokio::test]
async fn nullsafe_chain_suppresses_null_diagnostic() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
class Inner { public function value(): int { return 1; } }
class Outer { public function inner(): ?Inner { return null; } }
$outer = null;
$outer?->inner()?->value();
"#,
    )
    .await;
}

/// Contrast: the same definitely-null value WITHOUT the nullsafe operator
/// must still be flagged, proving the tests above exercise real suppression
/// rather than an analyzer that never checks null method/property access.
#[tokio::test]
async fn plain_method_call_on_null_is_flagged() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class Box { public function value(): int { return 1; } }
function _wrap(): void {
    $box = null;
    $box->value();
//  ^^^^^^^^^^^^^ error: Cannot call method value() on null
}
"#,
    )
    .await;
}

#[tokio::test]
async fn plain_property_fetch_on_null_is_flagged() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class Box { public int $value = 0; }
function _wrap(): void {
    $box = null;
    echo $box->value;
//       ^^^^^^^^^^^ error: Cannot access property $value on null
}
"#,
    )
    .await;
}

#[tokio::test]
async fn array_access_on_null_is_flagged() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function _wrap(): void {
    $arr = null;
    echo $arr['key'];
//       ^^^^^^^^^^^ error: Cannot access array on null
}
"#,
    )
    .await;
}

// ── match exhaustiveness & backed enum diagnostics ────────────────────────────

/// A `match` covering every case of a pure enum, with no `default`, is
/// exhaustive and must not be flagged.
#[tokio::test]
async fn match_on_enum_covering_all_cases_has_no_diagnostic() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
enum Suit {
    case Hearts;
    case Spades;
}
function label(Suit $s): string {
    return match ($s) {
        Suit::Hearts => 'hearts',
        Suit::Spades => 'spades',
    };
}
"#,
    )
    .await;
}

/// A non-exhaustive `match` on an enum with no `default` can throw
/// `UnhandledMatchError` at runtime for the missing case — must be flagged.
#[tokio::test]
async fn match_on_enum_missing_case_is_flagged() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
enum Suit {
    case Hearts;
    case Spades;
}
function label(Suit $s): string {
    return match ($s) {
//         ^ warning: Unhandled match condition
        Suit::Hearts => 'hearts',
    };
}
"#,
    )
    .await;
}

/// Contrast: a non-exhaustive `match` with a `default` arm covers every
/// remaining case at runtime and must not be flagged.
#[tokio::test]
async fn match_on_enum_missing_case_with_default_has_no_diagnostic() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
enum Suit {
    case Hearts;
    case Spades;
}
function label(Suit $s): string {
    return match ($s) {
        Suit::Hearts => 'hearts',
        default => 'other',
    };
}
"#,
    )
    .await;
}

/// KNOWN GAP, not fixable from php-lsp alone: mir's definition-collector
/// (`collect_file_definitions`, mir-analyzer `collector/enum.rs`) correctly
/// detects a backed enum case whose literal value doesn't match the backing
/// type — confirmed directly against `mir-cli analyze`, which reports
/// `BackedEnumCaseTypeMismatch` for this exact snippet. But php-lsp's
/// `DocumentStore::get_semantic_issues_salsa` only merges two issue sources
/// (`FileAnalyzer::analyze`'s body-analysis pass and `AnalysisSession::
/// class_issues` for inheritance/override checks) — it never reads
/// `collect_file_definitions(..).issues`, the collector-phase issues query,
/// so this diagnostic (and any other collector-time check) never reaches the
/// editor. Fixing this needs a new public accessor on mir's `AnalysisSession`
/// (`class_issues` only covers `ClassAnalyzer`, not the collector) — a mir
/// API change requiring a release, not a php-lsp-only fix.
#[tokio::test]
#[ignore = "collector-phase issues (e.g. BackedEnumCaseTypeMismatch) aren't wired into get_semantic_issues_salsa yet — needs a new mir AnalysisSession API + release"]
async fn backed_enum_case_value_type_mismatch_is_flagged() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
enum Suit: string {
    case Hearts = 1;
//  ^^^^^^^^^^^^^^^ error: Backed enum case Suit::Hearts has value of type int, but backing type is string
    case Spades = 'spades';
}
"#,
    )
    .await;
}

/// A closure declared outside any class, using `$this`, later rebound to an
/// object via `Closure::bind()`/`bindTo()`/`call()` is valid PHP.
#[tokio::test]
async fn closure_bind_rebinding_this_is_not_flagged() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
class Container {
    private int $value = 42;
}
$getter = function (): int {
    return $this->value;
};
$bound = Closure::bind($getter, new Container(), Container::class);
echo $bound();
"#,
    )
    .await;
}

// ── property visibility is not enforced on `->` access ──────────────────────

/// KNOWN GAP, not fixable from php-lsp alone: mir enforces visibility for
/// class constant access (`InaccessibleClassConstant`, checked in
/// mir-analyzer `expr/objects.rs`) and effectively for method calls (a
/// private/protected method looked up from outside the class isn't found at
/// all, surfacing as `UndefinedMethod` — see the positive controls below).
/// But there is no equivalent check for `->` property access: no
/// `InaccessibleProperty`-shaped variant exists in mir-issues, and the
/// property-fetch path never inspects the property's declared visibility.
/// Reading a `private`/`protected` property from outside its class silently
/// type-checks with no diagnostic at all. This pins today's (missing)
/// behavior; once mir grows the check, replace this with a `check_diagnostics`
/// assertion that the access IS flagged.
#[tokio::test]
async fn private_property_access_from_outside_class_is_not_yet_flagged() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
class Vault {
    private int $secret = 0;
}
function test(Vault $v): int {
    return $v->secret;
}
"#,
    )
    .await;
}

/// Same gap as above, for `protected` instead of `private`.
#[tokio::test]
async fn protected_property_access_from_outside_class_is_not_yet_flagged() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
class Vault {
    protected int $balance = 0;
}
function test(Vault $v): int {
    return $v->balance;
}
"#,
    )
    .await;
}

/// Positive control for the two gaps above: private/protected *method*
/// calls from outside the class ARE rejected today — just mislabeled, since
/// an inaccessible method fails lookup and reports as `UndefinedMethod`
/// rather than a dedicated inaccessibility diagnostic. Confirms the property
/// gap is a real hole in the visibility model, not evidence that php-lsp
/// never checks visibility at all.
#[tokio::test]
async fn private_method_access_from_outside_class_is_flagged_as_undefined() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class Vault {
    private function secret(): string { return 's'; }
}
function test(Vault $v): string {
    return $v->secret();
//         ^^^^^^^^^^^^ error: secret
}
"#,
    )
    .await;
}
