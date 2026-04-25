mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn signature_help_at_first_arg() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
function greet(string $name, int $count = 1): string { return $name; }
greet($0);
"#,
        )
        .await;
    expect!["▶ greet(string $name, int $count = 1)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_at_second_arg() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
function greet(string $name, int $count = 1): string { return $name; }
greet('x', $0);
"#,
        )
        .await;
    expect!["▶ greet(string $name, int $count = 1)  @param1"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_for_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
class Greeter {
    public function hello(string $name): string { return $name; }
}
$g = new Greeter();
$g->hello($0);
"#,
        )
        .await;
    expect!["▶ hello(string $name)  @param0"].assert_eq(&out);
}

/// Cursor inside the inner call of `outer(inner($0), 2)` must show `inner`'s
/// signature, not `outer`'s. A parser that tracks only one call frame will
/// show `outer` here — this test catches that regression.
#[tokio::test]
async fn signature_help_nested_call_shows_inner_function() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
function inner(int $x): int { return $x; }
function outer(int $a, int $b): void {}
outer(inner($0), 2);
"#,
        )
        .await;
    expect!["▶ inner(int $x)  @param0"].assert_eq(&out);
}

/// Calling a function with variadic params and multiple args: the active
/// parameter must stay pinned to the variadic param regardless of arg count.
#[tokio::test]
async fn signature_help_variadic_stays_active_past_first_arg() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
function sum(int ...$vals): int { return array_sum($vals); }
sum(1, 2, $0);
"#,
        )
        .await;
    expect!["▶ sum(int ...$vals)  @param0"].assert_eq(&out);
}

/// Signature help for a static method call `Cls::method($0)` must resolve to
/// that class's method, not fall back to a global function with the same name.
#[tokio::test]
async fn signature_help_static_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
class Math {
    public static function add(int $a, int $b): int { return $a + $b; }
}
Math::add($0);
"#,
        )
        .await;
    expect!["▶ add(int $a, int $b)  @param0"].assert_eq(&out);
}

/// Signature help for a zero-parameter function must not crash and must not
/// expose a stale `activeParameter` from a previous call in the same file.
#[tokio::test]
async fn signature_help_zero_param_function() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
function ping(): bool { return true; }
ping($0);
"#,
        )
        .await;
    expect!["▶ ping()"].assert_eq(&out);
}
