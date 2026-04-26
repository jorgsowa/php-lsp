//! Diagnostic coverage matrix using the caret annotation DSL.
//! Each test names the expectation inline with `// ^^^ severity: message`.

mod common;

use common::TestServer;

#[tokio::test]
async fn undefined_function_top_level() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function _wrap(): void {
    nonexistent_fn();
//  ^^^^^^^^^^^^^^^^ error: nonexistent_fn
}
"#,
    )
    .await;
}

#[tokio::test]
async fn undefined_function_inside_function() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function wrapper(): void {
    nonexistent_fn();
//  ^^^^^^^^^^^^^^^^ error: nonexistent_fn
}
"#,
    )
    .await;
}

#[tokio::test]
async fn undefined_function_inside_method() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class C {
    public function run(): void {
        nonexistent_fn();
//      ^^^^^^^^^^^^^^^^ error: nonexistent_fn
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn undefined_function_inside_namespaced_method() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
namespace LspTest;
class Broken {
    public function f(): void {
        nonexistent_fn();
//      ^^^^^^^^^^^^^^^^ error: nonexistent_fn
    }
}
"#,
    )
    .await;
}

/// Regression for issue #170: mir-analyzer must detect errors inside
/// namespaced class method bodies, not just top-level / non-namespaced code.
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
async fn undefined_class_in_new() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function _wrap(): void {
    $x = new UnknownClass();
//           ^^^^^^^^^^^^ error: UnknownClass
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
async fn workspace_diagnostic_returns_report() {
    let mut server = TestServer::new().await;
    server.open("ws_diag.php", "<?php\n$x = 1;\n").await;

    let resp = server.workspace_diagnostic().await;

    assert!(
        resp["error"].is_null(),
        "workspace/diagnostic error: {:?}",
        resp
    );
    let result = &resp["result"];
    let items = result["items"]
        .as_array()
        .expect("expected 'items' array in workspace diagnostic report");
    assert_eq!(
        items.len(),
        1,
        "expected exactly one item for the one opened file, got: {items:?}"
    );
}
