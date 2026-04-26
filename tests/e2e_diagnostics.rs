//! Diagnostic state-transition and regression tests.
//!
//! Scenario coverage (which diagnostics appear in what code) lives in
//! feature_diagnostics.rs. This file keeps only tests that exercise the *event
//! sequence* (open→change→verify), regression cases, and scope coverage that
//! feature_diagnostics.rs doesn't have.

mod common;

use common::TestServer;

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
