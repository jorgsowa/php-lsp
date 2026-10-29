//! Diagnostic coverage matrix using the caret annotation DSL.
//! Each test names the expectation inline with `// ^^^ severity: message`.

use super::*;

use expect_test::expect;
use serde_json::json;

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

#[tokio::test]
async fn argument_count_too_many_detected() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function takes_one(string $s): void {}
function wrap(): void {
    takes_one('a', 'b', 'c');
//                 ^^^ error: takes_one
}
"#,
        )
        .await;
}

#[tokio::test]
async fn argument_type_coercion_is_info_in_non_strict_mode() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function takes_string(string $s): void {}
function wrap(): void {
    takes_string(42);
//               ^^ info: takes_string
}
"#,
        )
        .await;
}

#[tokio::test]
async fn argument_type_mismatch_is_error_in_strict_mode() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
declare(strict_types=1);
function takes_string(string $s): void {}
function wrap(): void {
    takes_string(42);
//               ^^ error: takes_string
}
"#,
        )
        .await;
}

#[tokio::test]
async fn workspace_diagnostic_named_arguments() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ws_named_args.php",
            "<?php\nfunction foo(int $a, int $b): void {}\nfoo(a: 1, b: 2, a: 3);\n",
        )
        .await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));

    expect![[r#"
        ws_named_args.php
          2:16 foo() argument $a overwrites a previous argument [InvalidNamedArgument] (error)"#]]
    .assert_eq(&out);
}
