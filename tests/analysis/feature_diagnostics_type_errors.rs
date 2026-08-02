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
async fn duplicate_named_arg_in_constructor() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class Point {
    public function __construct(public int $x, public int $y) {}
}
new Point(x: 0, y: 1, x: 2);
//                    ^^^^ error: Point::__construct() has no parameter named $x
"#,
    )
    .await;
}

#[tokio::test]
async fn duplicate_named_arg_in_function_call() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function foo(int $a, int $b): void {}
foo(a: 1, b: 2, a: 3);
//              ^^^^ error: foo() has no parameter named $a
"#,
    )
    .await;
}

#[tokio::test]
async fn duplicate_named_arg_in_method_call() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class C {
    public function run(int $x, int $y): void {}
}
(new C())->run(x: 1, y: 2, x: 99);
//                         ^^^^^ error: run() has no parameter named $x
"#,
    )
    .await;
}

/// Two diagnostics fire on the same token here: the parser's own "positional
/// after named" syntax error, plus mir's arg-count pass independently
/// flagging the same positional argument as an unmatched named one — its
/// message leaks the internal placeholder name `#2` (mir-analyzer
/// `call/args/counts.rs`) rather than suppressing itself once the parse
/// error already covers this argument. Upstream mir issue, not fixable from
/// php-lsp's diagnostic wiring; this test pins current (imperfect) behavior.
#[tokio::test]
async fn positional_after_named_arg() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_diagnostics(
        r#"<?php
function bar(int $a, int $b): void {}
bar(a: 1, 2);
//        ^ error: cannot use positional argument after named argument
//        ^ error: bar() has no parameter named $#2
"#,
    )
    .await;
}

#[tokio::test]
async fn valid_named_args_produce_no_diagnostic() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function greet(string $name, int $times): void {}
greet(name: 'Alice', times: 3);
"#,
    )
    .await;
}

/// Spreading a string-keyed array as a call's sole argument (`f(...$args)`)
/// binds each entry to the parameter of the same name — valid PHP 8.1+
/// named-argument syntax (confirmed: `php -l` passes and it runs, printing
/// "BobBob"). mir-analyzer's `expand_sole_spread_arg` now recognizes
/// string-keyed spreads and binds by parameter name instead of falling back
/// to a merged-union check against the first parameter.
#[tokio::test]
async fn spread_array_with_string_keys_as_named_args_not_flagged() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
function greet(string $name, int $times = 1): string {
    return str_repeat($name, $times);
}
function test(): string {
    $args = ['name' => 'Bob', 'times' => 2];
    return greet(...$args);
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
          2:16 foo() has no parameter named $a [InvalidNamedArgument] (error)"#]]
    .assert_eq(&out);
}
