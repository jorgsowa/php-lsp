//! Diagnostic emission tests — expectations live next to the offending code
//! via inline `// ^^^` annotations. See `tests/common/fixture.rs` for the
//! annotation syntax.
//!
//! State-transition tests (did_change republish, diagnostics clearing) stay on
//! the raw API since they're about the *event sequence*, not a single payload.

mod common;

use common::TestServer;

#[tokio::test]
async fn did_open_reports_undefined_function_in_top_level_wrapper() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function f(): void {
    nonexistent_function();
//  ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
}
"#,
        )
        .await;
}

#[tokio::test]
async fn did_open_reports_undefined_class_instantiation() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function f(): void {
    $x = new UnknownClass();
//           ^^^^^^^^^^^^ error: UnknownClass
}
"#,
        )
        .await;
}

#[tokio::test]
async fn diagnostics_published_on_did_change_for_undefined_function() {
    let mut server = TestServer::new().await;
    server.open("change_test.php", "<?php\n").await;

    let notif = server
        .change("change_test.php", 2, "<?php\nnonexistent_function();\n")
        .await;
    let has = notif["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"].as_str() == Some("UndefinedFunction"));
    assert!(has, "expected UndefinedFunction after didChange: {notif:?}");
}

#[tokio::test]
async fn diagnostics_clear_when_code_is_fixed() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("fix_test.php", "<?php\nnonexistent_function();\n")
        .await;
    assert!(
        !notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected at least one diagnostic for broken code: {notif:?}"
    );

    let notif = server.change("fix_test.php", 2, "<?php\n").await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap().clone();
    assert!(
        diags.is_empty(),
        "diagnostics must be empty after fixing the code: {diags:?}"
    );
}

/// The mir analyzer must flag undefined function calls at every scope: inside
/// a class method, and inside a method of a namespaced class. (Plain-function
/// scope is covered by `did_open_reports_undefined_function_in_top_level_wrapper`.)
#[tokio::test]
async fn undefined_function_detected_in_class_method() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
class A {
    public function f(): void {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
    }
}
"#,
        )
        .await;
}

#[tokio::test]
async fn undefined_function_detected_in_namespaced_class_method() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
namespace LspTest;
class Broken {
    public function f(): void {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
    }
}
"#,
        )
        .await;
}

/// Regression for issue #177 — deprecated-call warnings must appear on did_open,
/// not only after the first did_change.
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
    let has_deprecated = diags.iter().any(|d| {
        d["message"]
            .as_str()
            .map(|m| m.contains("oldFunc") && m.contains("eprecated"))
            .unwrap_or(false)
    });
    assert!(
        has_deprecated,
        "expected a deprecated warning on did_open, got: {diags:?}"
    );
}

/// Regression for issue #170 — undefined function and class references in a
/// method body of a namespaced class must both be reported.
#[tokio::test]
async fn issue_170_undefined_function_and_class_in_method_body() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
namespace LspTest;

class Broken
{
    public int $count = 0;

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

/// Static methods are a separate scope; the analyzer must descend into them.
#[tokio::test]
async fn undefined_function_detected_in_static_method() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
class Factory {
    public static function build(): void {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
    }
}
"#,
        )
        .await;
}

/// Arrow functions (`fn() => expr`) are a PHP 8.0 construct; the analyzer
/// must walk their bodies rather than treating them as opaque.
#[tokio::test]
async fn undefined_function_detected_in_arrow_function() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
$fn = fn() => nonexistent_function();
//            ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
"#,
        )
        .await;
}

/// Traits carry their own method bodies; the analyzer must analyze them just
/// like class methods.
///
/// Currently ignored: `mir-analyzer` 0.8.x does not descend into trait method
/// bodies, so no diagnostics are emitted for undefined calls inside traits.
/// Remove `#[ignore]` when mir-analyzer covers trait scopes.
#[ignore = "mir-analyzer gap: trait method bodies are not analyzed"]
#[tokio::test]
async fn undefined_function_detected_in_trait_method() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
trait Auditable {
    public function audit(): void {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
    }
}
"#,
        )
        .await;
}

/// A closure captures an outer scope but still gets its own scope for local
/// variables. Undefined function calls inside closures must be reported.
#[tokio::test]
async fn undefined_function_detected_in_closure() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
$fn = function() {
    nonexistent_function();
//  ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
};
"#,
        )
        .await;
}

/// Passing too few arguments to a user-defined function is flagged as
/// `InvalidArgument` (the same code used for type mismatches). The diagnostic
/// spans the whole call expression.
#[tokio::test]
async fn argument_count_too_few_detected() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function needs_two(string $a, string $b): void {}
function wrap(): void {
    needs_two('x');
//  ^^^^^^^^^^^^^^ error: needs_two
}
"#,
        )
        .await;
}

/// Passing a value of the wrong type to a typed parameter emits `InvalidArgument`.
/// The diagnostic range covers the offending argument expression.
#[tokio::test]
async fn argument_type_mismatch_detected() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function takes_string(string $s): void {}
function wrap(): void {
    takes_string(42);
//               ^^ error: takes_string
}
"#,
        )
        .await;
}

/// Passing too *many* arguments to a user-defined function — a genuine arity
/// over-application — is not yet detected by `mir-analyzer`. Remove `#[ignore]`
/// once the analyzer covers this case.
#[ignore = "mir-analyzer gap: too-many-arguments not detected"]
#[tokio::test]
async fn argument_count_too_many_detected() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function takes_one(string $s): void {}
function wrap(): void {
    takes_one('a', 'b', 'c');
//  ^^^^^^^^^^^^^^^^^^^^^^^^^ error: takes_one
}
"#,
        )
        .await;
}
