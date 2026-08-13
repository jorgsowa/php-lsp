//! Variable-scoped reference tests: `$var` references must be confined to the
//! enclosing function/method and must not bleed into unrelated scopes.

use super::*;

/// References for a function parameter variable must be scoped to that
/// function and not include the same-named variable in a sibling function.
#[tokio::test]
async fn references_variable_in_function_scoped_to_function() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function foo(int $x$0): int {
    //           ^^ def
    $y = $x + 1;
    //   ^^ ref
    return $x;
    //     ^^ ref
}
function bar(int $x): int { return $x; }
"#,
    )
    .await;
}

/// All three occurrences of a typed param must appear, and the declaration
/// span must start at `$`, not at the type annotation.
#[tokio::test]
async fn references_variable_typed_param_includes_param_decl() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function process(string $val$0): string {
    //                  ^^^^ def
    $out = strtoupper($val);
    //                ^^^^ ref
    return $val;
    //     ^^^^ ref
}
"#,
    )
    .await;
}

/// `$key` in a foreach loop shares function scope with code after the loop —
/// all occurrences within the function are returned, but a same-named
/// variable in a different function is excluded.
#[tokio::test]
async fn references_variable_foreach_key_scoped_to_function() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function iter(array $items): void {
    foreach ($items as $ke$0y => $child) {
        //             ^^^^ def
        echo $key;
        //   ^^^^ ref
    }
    $key = 'reset';
//  ^^^^ ref
}
function other(): void { $key = 'unrelated'; }
"#,
    )
    .await;
}

/// References in `first()` must not include the `$result` in `second()`.
#[tokio::test]
async fn references_variable_different_functions_not_mixed() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function first(): void {
    $result$0 = 1;
//  ^^^^^^^ def
    echo $result;
//       ^^^^^^^ ref
}
function second(): void {
    $result = 2;
    echo $result;
}
"#,
    )
    .await;
}

/// Method parameter references must be scoped to the method and not include
/// the same-named parameter in a sibling method.
#[tokio::test]
async fn references_variable_method_body_scoped_to_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Calc {
    public function add(int $a$0, int $b): int {
        //                  ^^ def
        return $a + $b;
        //     ^^ ref
    }
    public function mul(int $a, int $b): int { return $a * $b; }
}
"#,
    )
    .await;
}

/// Cursor on a body-assigned variable (not a parameter) returns all occurrences
/// in scope; a same-named variable in another function is excluded.
#[tokio::test]
async fn references_variable_body_assigned_scoped_to_function() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function build(): string {
    $out$0 = 'start';
//  ^^^^ def
    $out .= ' middle';
//  ^^^^ ref
    return $out;
//         ^^^^ ref
}
function other(): string { $out = 'x'; return $out; }
"#,
    )
    .await;
}

/// Foreach value variable (no key) must be found and scoped to the function.
#[tokio::test]
async fn references_variable_foreach_value_only() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function sum(array $nums): int {
    $total = 0;
    foreach ($nums as $nu$0m) {
        //            ^^^^ def
        $total += $num;
        //        ^^^^ ref
    }
    return $total;
}
"#,
    )
    .await;
}

/// Catch-block exception variable body usage must be found and must not bleed
/// into the same-named variable in another function's catch block.
/// Note: the catch binding itself (`catch (Exception $e)`) is not collected
/// as a "declaration" by the scope walker — only body usages are returned.
#[tokio::test]
async fn references_variable_catch_exception_scoped_to_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function run(): void {
    try {
        doStuff();
    } catch (Exception $e$0) {
        log($e->getMessage());
        //  ^^ ref
    }
}
function other(): void {
    try { doOther(); } catch (Exception $e) { log($e); }
}
"#,
    )
    .await;
}

/// An untyped (plain) parameter `function foo($x)` must be found correctly;
/// `p.span` already starts at `$` so no narrowing is needed.
#[tokio::test]
async fn references_variable_untyped_param() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function double($n$0): int {
    //          ^^ def
    return $n * 2;
    //     ^^ ref
}
function triple($n): int { return $n * 3; }
"#,
    )
    .await;
}

/// Variable used inside a static method must be scoped to that method.
#[tokio::test]
async fn references_variable_static_method_scoped() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Factory {
    public static function make(string $type$0): object {
        //                             ^^^^^ def
        return match ($type) {
        //            ^^^^^ ref
            'foo' => new Foo(),
            default => new stdClass(),
        };
    }
    public static function list(string $type): array { return []; }
}
"#,
    )
    .await;
}

/// A nested arrow function parameter must own its own `$x` references and must
/// not merge with an outer `$x` when the cursor is inside the arrow body.
#[tokio::test]
async fn references_variable_arrow_param_shadowing_owns_nested_scope() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function demo(int $x): int {
    $outer = $x;
    $calc = fn(int $x$0) => $x + 1;
//                 ^^ def
//                        ^^ ref
    return $calc(2) + $outer + $x;
}
"#,
    )
    .await;
}

/// A closure with its own local variable must be treated as a nested
/// variable owner instead of being merged into the enclosing function scope.
#[tokio::test]
async fn references_variable_closure_scope_owns_shadowed_name() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function demo(int $x): int {
    $outer = $x;
    $calc = function () use ($outer): int {
        $x = $outer + 1;
//      ^^ def
        return $x$0;
//             ^^ ref
    };
    return $calc(2) + $x;
}
"#,
    )
    .await;
}
