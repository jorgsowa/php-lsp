//! Code actions for arrow function ↔ closure conversions:
//! - "Convert to arrow function": closure with single `return` → arrow function
//! - "Convert to closure": arrow function → closure with `{ return …; }`

use super::*;
use expect_test::expect;

// --- Offered ---

#[tokio::test]
async fn arrow_fn_offered_for_simple_closure() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
$fn = $0function() { return 42; }$0;
"#,
        )
        .await;
    assert!(
        out.contains("Convert to arrow function"),
        "expected action in: {out}"
    );
}

#[tokio::test]
async fn arrow_fn_offered_when_cursor_anywhere_in_closure() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    // Cursor in the middle of the body, not on `function` keyword.
    let out = s
        .check_code_actions(
            r#"<?php
$fn = function() { return $042; };
"#,
        )
        .await;
    assert!(
        out.contains("Convert to arrow function"),
        "expected action in: {out}"
    );
}

// --- Not offered ---

#[tokio::test]
async fn arrow_fn_not_offered_for_multi_statement_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
$fn = $0function($x) {
    $y = $x * 2;
    return $y;
}$0;
"#,
        )
        .await;
    assert!(
        !out.contains("Convert to arrow function"),
        "should not offer for multi-statement body, got: {out}"
    );
}

#[tokio::test]
async fn arrow_fn_not_offered_when_body_is_not_return() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
$fn = $0function($x) { echo $x; }$0;
"#,
        )
        .await;
    assert!(
        !out.contains("Convert to arrow function"),
        "should not offer when body is not return, got: {out}"
    );
}

#[tokio::test]
async fn arrow_fn_not_offered_for_by_ref_use_capture() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
$counter = 0;
$fn = $0function() use (&$counter) { return $counter; }$0;
"#,
        )
        .await;
    assert!(
        !out.contains("Convert to arrow function"),
        "should not offer for by-ref use capture, got: {out}"
    );
}

// --- Applied edits ---

#[tokio::test]
async fn arrow_fn_converts_no_param_closure() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0function() { return 42; }$0;
"#,
            "Convert to arrow function",
        )
        .await;
    expect![[r#"
        <?php
        $fn = fn() => 42;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn arrow_fn_converts_closure_with_params() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0function(int $x, int $y) { return $x + $y; }$0;
"#,
            "Convert to arrow function",
        )
        .await;
    expect![[r#"
        <?php
        $fn = fn(int $x, int $y) => $x + $y;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn arrow_fn_converts_closure_with_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0function(string $s): string { return strtoupper($s); }$0;
"#,
            "Convert to arrow function",
        )
        .await;
    expect![[r#"
        <?php
        $fn = fn(string $s): string => strtoupper($s);
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn arrow_fn_converts_static_closure() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0static function(int $n): int { return $n * 2; }$0;
"#,
            "Convert to arrow function",
        )
        .await;
    expect![[r#"
        <?php
        $fn = static fn(int $n): int => $n * 2;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn arrow_fn_drops_value_use_clause() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$base = 10;
$fn = $0function(int $x) use ($base) { return $x + $base; }$0;
"#,
            "Convert to arrow function",
        )
        .await;
    expect![[r#"
        <?php
        $base = 10;
        $fn = fn(int $x) => $x + $base;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn arrow_fn_converts_closure_inside_array_map() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$result = array_map($0function(int $n) { return $n * $n; }$0, $items);
"#,
            "Convert to arrow function",
        )
        .await;
    expect![[r#"
        <?php
        $result = array_map(fn(int $n) => $n * $n, $items);
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn arrow_fn_converts_innermost_nested_closure() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$outer = function(int $a) {
    return $0function(int $b) { return $a + $b; }$0;
};
"#,
            "Convert to arrow function",
        )
        .await;
    expect![[r#"
        <?php
        $outer = function(int $a) {
            return fn(int $b) => $a + $b;
        };
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn arrow_fn_with_nullable_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0function(?string $s): ?string { return $s; }$0;
"#,
            "Convert to arrow function",
        )
        .await;
    expect![[r#"
        <?php
        $fn = fn(?string $s): ?string => $s;
    "#]]
    .assert_eq(&out);
}

// ── Convert to closure ────────────────────────────────────────────────────────

// --- Offered ---

#[tokio::test]
async fn to_closure_offered_for_simple_arrow_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
$fn = $0fn() => 42$0;
"#,
        )
        .await;
    assert!(
        out.contains("Convert to closure"),
        "expected action in: {out}"
    );
}

#[tokio::test]
async fn to_closure_offered_when_cursor_inside_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
$fn = fn($x) => $0$x * 2;
"#,
        )
        .await;
    assert!(
        out.contains("Convert to closure"),
        "expected action in: {out}"
    );
}

// --- Not offered ---

#[tokio::test]
async fn to_closure_not_offered_when_cursor_is_outside() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    // Cursor is before the fn keyword
    let out = s
        .check_code_actions(
            r#"<?php
$fn$0 = fn() => 42;
"#,
        )
        .await;
    assert!(
        !out.contains("Convert to closure"),
        "should not offer when cursor is outside arrow function, got: {out}"
    );
}

// --- Applied edits ---

#[tokio::test]
async fn to_closure_converts_no_param_arrow() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0fn() => 42$0;
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $fn = function() { return 42; };
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn to_closure_converts_arrow_with_params() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0fn(int $x, int $y) => $x + $y$0;
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $fn = function(int $x, int $y) { return $x + $y; };
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn to_closure_converts_arrow_with_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0fn(string $s): string => strtoupper($s)$0;
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $fn = function(string $s): string { return strtoupper($s); };
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn to_closure_converts_static_arrow() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0static fn(int $n): int => $n * 2$0;
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $fn = static function(int $n): int { return $n * 2; };
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn to_closure_converts_arrow_inside_array_map() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$result = array_map($0fn(int $n) => $n * $n$0, $items);
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $result = array_map(function(int $n) { return $n * $n; }, $items);
    "#]]
    .assert_eq(&out);
}

/// The inner arrow function reads `$a` from the outer scope — arrow functions
/// auto-capture that for free, but a plain closure needs an explicit
/// `use ($a)` or `$a` is undefined at runtime. Converting must synthesize it.
#[tokio::test]
async fn to_closure_converts_innermost_nested_arrow() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$outer = fn($a) => $0fn($b) => $a + $b$0;
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $outer = fn($a) => function($b) use ($a) { return $a + $b; };
    "#]]
    .assert_eq(&out);
}

/// Same use-clause requirement as above, with the outer scope being a plain
/// closure's parameter rather than another arrow function's.
#[tokio::test]
async fn to_closure_converts_arrow_inside_closure_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$outer = function(int $a) {
    return $0fn(int $b) => $a + $b$0;
};
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $outer = function(int $a) {
            return function(int $b) use ($a) { return $a + $b; };
        };
    "#]]
    .assert_eq(&out);
}

/// Multiple distinct outer-scope reads must all land in the use clause, in
/// first-appearance order, deduped (`$a` appears twice).
#[tokio::test]
async fn to_closure_captures_multiple_outer_variables() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function make($a, $b, $c) {
    return $0fn($x) => $a + $b * $x + $a - $c$0;
}
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        function make($a, $b, $c) {
            return function($x) use ($a, $b, $c) { return $a + $b * $x + $a - $c; };
        }
    "#]]
    .assert_eq(&out);
}

/// `$this` is always implicitly available in a non-static closure — it must
/// never be added to the synthesized `use` clause (`use ($this)` is a PHP
/// fatal error).
#[tokio::test]
async fn to_closure_does_not_capture_this() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Greeter {
    public string $name = "World";
    public function make() {
        return $0fn() => "Hello, {$this->name}"$0;
    }
}
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        class Greeter {
            public string $name = "World";
            public function make() {
                return function() { return "Hello, {$this->name}"; };
            }
        }
    "#]]
    .assert_eq(&out);
}

/// Superglobals (`$_GET`, `$_SERVER`, `$GLOBALS`, ...) are part of the PHP
/// runtime and always in scope — `use ($_GET)` is a compile-time fatal error
/// ("Cannot use $_GET as lexical variable as it is a superglobal"). They
/// must never be added to the synthesized `use` clause.
#[tokio::test]
async fn to_closure_does_not_capture_superglobal() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0fn() => $_GET['id']$0;
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $fn = function() { return $_GET['id']; };
    "#]]
    .assert_eq(&out);
}

/// A superglobal alongside a genuine outer variable: only the outer
/// variable belongs in the `use` clause.
#[tokio::test]
async fn to_closure_captures_only_non_superglobal_variable() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function make($prefix) {
    return $0fn() => $prefix . $_SERVER['HTTP_HOST']$0;
}
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        function make($prefix) {
            return function() use ($prefix) { return $prefix . $_SERVER['HTTP_HOST']; };
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn to_closure_with_nullable_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$fn = $0fn(?string $s): ?string => $s$0;
"#,
            "Convert to closure",
        )
        .await;
    expect![[r#"
        <?php
        $fn = function(?string $s): ?string { return $s; };
    "#]]
    .assert_eq(&out);
}
