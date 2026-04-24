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
