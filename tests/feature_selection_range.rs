mod common;

use common::TestServer;
use expect_test::expect;

// ── basic shape ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn empty_php_file_returns_file_range_only() {
    let mut s = TestServer::new().await;
    let out = s.check_selection_range("<?php$0\n").await;
    expect!["0:0-1:0"].assert_eq(&out);
}

#[tokio::test]
async fn cursor_outside_any_construct_returns_only_file_range() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range("<?php\n$0// only a comment\n")
        .await;
    expect!["0:0-2:0"].assert_eq(&out);
}

#[tokio::test]
async fn end_character_is_real_line_length_not_u32_max() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range("<?php\nfunction hello(): void {$0}\n")
        .await;
    expect![[r#"
        1:0-1:25
        0:0-2:0"#]]
    .assert_eq(&out);
}

// ── statement granularity ────────────────────────────────────────────────────

#[tokio::test]
async fn cursor_in_function_body_includes_function_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function greet() {
    echo $0'hi';
}
"#,
        )
        .await;
    expect![[r#"
        2:9-2:13
        2:4-2:14
        1:0-3:1
        0:0-4:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn cursor_in_method_body_walks_class_method_body_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
class Foo {
    public function bar() {
        echo $0 1;
    }
}
"#,
        )
        .await;
    expect![[r#"
        3:8-3:16
        2:4-4:5
        1:0-5:1
        0:0-6:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn cursor_on_class_member_outside_method_body() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
class Foo {
    public int $x$0 = 1;
    public function bar() {}
}
"#,
        )
        .await;
    expect![[r#"
        2:4-2:21
        1:0-4:1
        0:0-5:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn interface_member_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
interface Greeter {
    public function gree$0t(): string;
}
"#,
        )
        .await;
    expect![[r#"
        2:4-2:36
        1:0-3:1
        0:0-4:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn trait_method_body_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
trait Greets {
    public function hi(): void {
        echo$0 'hi';
    }
}
"#,
        )
        .await;
    expect![[r#"
        3:8-3:18
        2:4-4:5
        1:0-5:1
        0:0-6:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn enum_case_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
enum Color {
    case R$0ed;
    case Green;
}
"#,
        )
        .await;
    expect![[r#"
        2:4-2:13
        1:0-4:1
        0:0-5:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn enum_method_body_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
enum Color {
    case Red;
    public function label(): string {
        return$0 'red';
    }
}
"#,
        )
        .await;
    expect![[r#"
        4:8-4:21
        3:4-5:5
        1:0-6:1
        0:0-7:0"#]]
    .assert_eq(&out);
}

// ── nested control flow ──────────────────────────────────────────────────────

#[tokio::test]
async fn nested_if_while_foreach_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(array $xs): void {
    if (count($xs) > 0) {
        foreach ($xs as $x) {
            while ($x > 0) {
                echo$0 $x;
                $x--;
            }
        }
    }
}
"#,
        )
        .await;
    expect![[r#"
        5:16-5:24
        4:12-7:13
        4:27-7:13
        3:8-8:9
        3:28-8:9
        2:4-9:5
        2:24-9:5
        1:0-10:1
        0:0-11:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn try_catch_finally_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(): void {
    try {
        echo 1;
    } catch (Throwable $e) {
        echo$0 2;
    } finally {
        echo 3;
    }
}
"#,
        )
        .await;
    expect![[r#"
        5:8-5:15
        4:12-6:5
        2:4-8:5
        1:0-9:1
        0:0-10:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn try_finally_block_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(): void {
    try {
        echo 1;
    } finally {
        echo$0 2;
    }
}
"#,
        )
        .await;
    expect![[r#"
        5:8-5:15
        2:4-6:5
        1:0-7:1
        0:0-8:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn for_and_do_while_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(): void {
    for ($i = 0; $i < 10; $i++) {
        do {
            echo$0 $i;
        } while ($i < 5);
    }
}
"#,
        )
        .await;
    expect![[r#"
        4:12-4:20
        3:8-5:25
        3:11-5:9
        2:4-6:5
        2:32-6:5
        1:0-7:1
        0:0-8:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn elseif_branch_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(int $x): void {
    if ($x > 1) {
        echo 1;
    } elseif ($x > 0) {
        echo$0 2;
    } else {
        echo 3;
    }
}
"#,
        )
        .await;
    expect![[r#"
        5:8-5:15
        4:13-6:5
        4:22-6:5
        2:4-8:5
        1:0-9:1
        0:0-10:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn else_branch_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(int $x): void {
    if ($x > 0) {
        echo 1;
    } else {
        echo$0 2;
    }
}
"#,
        )
        .await;
    expect![[r#"
        5:8-5:15
        4:11-6:5
        2:4-6:5
        1:0-7:1
        0:0-8:0"#]]
    .assert_eq(&out);
}

// ── namespace ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn braced_namespace_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
namespace App {
    function inner(): void {
        echo$0 1;
    }
}
"#,
        )
        .await;
    expect![[r#"
        3:8-3:15
        2:4-4:5
        1:0-5:1
        0:0-6:0"#]]
    .assert_eq(&out);
}

// ── UTF-16 column semantics ──────────────────────────────────────────────────

#[tokio::test]
async fn utf16_column_uses_utf16_units_for_supplementary_chars() {
    let mut s = TestServer::new().await;
    // "🦀" is one UTF-16 surrogate pair = 2 code units. The cursor sits after
    // it inside a string literal; the chain should use UTF-16 columns
    // throughout, including the file-level outermost range.
    let out = s
        .check_selection_range("<?php\nfunction f(): void { $s = '🦀$0'; }\n")
        .await;
    expect![[r#"
        1:26-1:30
        1:21-1:30
        1:21-1:31
        1:0-1:33
        0:0-2:0"#]]
    .assert_eq(&out);
}

// ── multi-position requests ─────────────────────────────────────────────────

#[tokio::test]
async fn multiple_positions_yield_independent_chains() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range_at(
            r#"<?php
function a() { echo 1; }
function b() { echo 2; }
"#,
            vec![(1, 16), (2, 16)],
        )
        .await;
    expect![[r#"
        1:15-1:22
        1:0-1:24
        0:0-3:0
        ---
        2:15-2:22
        2:0-2:24
        0:0-3:0"#]]
    .assert_eq(&out);
}

// ── parent / child ordering invariant ────────────────────────────────────────

#[tokio::test]
async fn chain_is_strictly_nested() {
    let mut s = TestServer::new().await;
    // Locks the exact ordering for a deeply nested cursor; the snapshot is
    // also a regression guard against the parent-must-cover-child invariant.
    let out = s
        .check_selection_range(
            r#"<?php
class C {
    public function m(): void {
        if (true) {
            echo$0 1;
        }
    }
}
"#,
        )
        .await;
    expect![[r#"
        4:12-4:19
        3:8-5:9
        3:18-5:9
        2:4-6:5
        1:0-7:1
        0:0-8:0"#]]
    .assert_eq(&out);
}

// ── switch / match / expression / parameter granularity ─────────────────────

#[tokio::test]
async fn switch_case_body_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(int $x): void {
    switch ($x) {
        case 1:
            echo$0 1;
            break;
        default:
            echo 0;
    }
}
"#,
        )
        .await;
    expect![[r#"
        4:12-4:19
        3:8-5:18
        2:4-8:5
        1:0-9:1
        0:0-10:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn binary_expression_inside_return_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function add(int $x): int {
    return $x +$0 1;
}
"#,
        )
        .await;
    expect![[r#"
        2:11-2:17
        2:4-2:18
        1:0-3:1
        0:0-4:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn match_arm_body_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(int $x): string {
    return match ($x) {
        1 => 'on$0e',
        2 => 'two',
        default => 'other',
    };
}
"#,
        )
        .await;
    expect![[r#"
        3:13-3:18
        3:8-3:18
        2:4-6:6
        2:11-6:5
        1:0-7:1
        0:0-8:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn function_call_argument_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(): void {
    str_pad('x', 1$0 + 1, '0');
}
"#,
        )
        .await;
    expect![[r#"
        2:17-2:22
        2:4-2:28
        2:4-2:29
        1:0-3:1
        0:0-4:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn array_element_chain() {
    let mut s = TestServer::new().await;
    let out = s
        .check_selection_range(
            r#"<?php
function f(): array {
    return ['a' => 1, 'b'$0 => 2, 'c' => 3];
}
"#,
        )
        .await;
    expect![[r#"
        2:22-2:30
        2:11-2:41
        2:4-2:42
        1:0-3:1
        0:0-4:0"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn parameter_chain() {
    let mut s = TestServer::new().await;
    // Cursor lands inside the type hint of the second parameter so the
    // chain pulls in the parameter span (not just the function).
    let out = s
        .check_selection_range("<?php\nfunction f(int $x, str$0ing $y): void {}\n")
        .await;
    expect![[r#"
        1:19-1:28
        1:0-1:38
        0:0-2:0"#]]
    .assert_eq(&out);
}
