//! documentHighlight coverage using the `ref`/`read`/`write` annotation tags.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn highlight_variable_occurrences_within_function() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function f(): void {
    $name = 'x';
//  ^^^^^ write
    echo $na$0me;
//       ^^^^^ read
    $name .= '!';
//  ^^^^^ write
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_method_call_within_same_file() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
class Greeter {
    public function hel$0lo(): void {}
    //              ^^^^^ ref
}
$g = new Greeter();
$g->hello();
//  ^^^^^ ref
$g->hello();
//  ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_variable_inside_enum_method() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
enum Status {
    public function label(
        $a$0rg
    //  ^^^^ write
    ) {
        return $arg + 1;
    //         ^^^^ read
    }
}
"#,
    )
    .await;
}

/// $arg in outer scope must not bleed into the enum method's highlight set.
#[tokio::test]
async fn highlight_enum_method_does_not_bleed_outer_scope() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
$arg = 0;
enum Status {
    public function label(
        $a$0rg
    //  ^^^^ write
    ) {
        return $arg + 1;
    //         ^^^^ read
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_cursor_on_string_literal_returns_empty() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
echo 'hel$0lo';
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    expect!["<no highlights>"].assert_eq(&render_document_highlight(&resp));
}

#[tokio::test]
async fn highlight_variable_assignment_and_read_in_scope() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo() {
    $x$0 = 1;
//  ^^ write
    echo $x;
//       ^^ read
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_variable_does_not_cross_function_scope() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo() {
    $x$0 = 1;
//  ^^ write
}
function bar() {
    $x = 2;
}
"#,
    )
    .await;
}

/// An arrow function's own parameter shadows an outer variable of the same
/// name — the two must not be merged into one highlight group, even though
/// arrow functions otherwise auto-capture (and so are normally traversed).
#[tokio::test]
async fn highlight_variable_does_not_cross_arrow_function_param_shadow() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo() {
    $x$0 = 1;
//  ^^ write
    $inner = fn($x) => $x + 1;
    echo $x;
//       ^^ read
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_variable_compound_assignment() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo() {
    $x$0 = 1;
//  ^^ write
    $x .= '!';
//  ^^ write
    echo $x;
//       ^^ read
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_class_name_decl_and_instantiation() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
class Fo$0o {}
//    ^^^ ref
$x = new Foo();
//       ^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_range_spans_full_word_width() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
function gree$0t() {}
greet();
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    expect![[r#"
        1:9-1:14 [text]
        2:0-2:5 [text]"#]]
    .assert_eq(&render_document_highlight(&resp));
}

#[tokio::test]
async fn highlight_cursor_beyond_line_end_returns_empty() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
function greet() {}$0
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    expect!["<no highlights>"].assert_eq(&render_document_highlight(&resp));
}

#[tokio::test]
async fn highlight_static_method_call() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
class Calc {
    public static function add$0() { return 1 + 2; }
    //                     ^^^ ref
}
Calc::add();
//    ^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_class_constant_decl_and_reference() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
class Foo {
    const BA$0R = 1;
    //    ^^^ ref
}
echo Foo::BAR;
//        ^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_variable_increment_operators() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo() {
    $x$0++;
//  ^^ ref
    ++$x;
//    ^^ ref
    --$x;
//    ^^ ref
    $x--;
//  ^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_foreach_key_binding_and_use() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo($arr) {
    foreach ($arr as $k$0ey => $value) {
//                   ^^^^ write
        echo $key;
//           ^^^^ read
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_foreach_value_binding_and_use() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo($arr) {
    foreach ($arr as $v$0alue) {
//                   ^^^^^^ write
        echo $value;
//           ^^^^^^ read
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_function_parameter() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo($n$0ame) {
//           ^^^^^ write
    echo $name;
//       ^^^^^ read
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_class_constant_multiple_refs() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
class Status {
    const AC$0TIVE = 1;
    //    ^^^^^^ ref
    const INACTIVE = 0;

    public function check() {
        if ($this->value === Status::ACTIVE) {
//                                   ^^^^^^ ref
            return Status::ACTIVE;
//                         ^^^^^^ ref
        }
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_this_variable() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
class Foo {
    public function bar() {
        $th$0is->baz();
//      ^^^^^ read
        $this->qux();
//      ^^^^^ read
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn highlight_symbol_inside_string_not_highlighted() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
function foo$0() {}
foo();
echo 'call foo here';
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    use expect_test::expect;
    expect![[r#"
        1:9-1:12 [text]
        2:0-2:3 [text]"#]]
    .assert_eq(&render_document_highlight(&resp));
}

/// Array-destructuring assignment: `[$a, $b] = expr` — both `$a` and `$b` on
/// the LHS are write positions. Previously they were tagged READ because the
/// `ExprKind::Assign` handler only checked for a direct `Variable` target and
/// fell into `visit_expr` for any other lhs, which produces READ.
#[tokio::test]
async fn highlight_array_destructuring_lhs_is_write() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo(): void {
    [$fi$0rst, $second] = [1, 2];
//   ^^^^^^ write
    echo $first;
//       ^^^^^^ read
}
"#,
    )
    .await;
}

/// `list($a, $b) = expr` is the classical destructuring form; the parser
/// normalises it to the same array-destructuring AST as `[$a, $b] = expr`.
#[tokio::test]
async fn highlight_list_destructuring_lhs_is_write() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo(): void {
    list($ke$0y, $val) = ['k', 'v'];
//       ^^^^ write
    echo $key;
//       ^^^^ read
}
"#,
    )
    .await;
}

/// `global $x` is a write (it binds the global into the local scope) and must
/// be highlighted as WRITE, not READ.
#[tokio::test]
async fn highlight_global_declaration_is_write() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo(): void {
    global $co$0nfig;
//         ^^^^^^^ write
    echo $config['debug'];
//       ^^^^^^^ read
}
"#,
    )
    .await;
}

/// `static $x = val` declares and initialises a persistent local; the declaration
/// must appear as a WRITE in highlights (previously it was missing entirely because
/// `walk_stmt` for `StaticVar` only visited the default expression, not the name).
#[tokio::test]
async fn highlight_static_variable_declaration_is_write() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function counter(): int {
    static $co$0unt = 0;
//         ^^^^^^ write
    $count++;
//  ^^^^^^ write
    return $count;
//         ^^^^^^ read
}
"#,
    )
    .await;
}

/// Nested array destructuring: `[[$a, $b], $c] = ...` — all three variables
/// are write positions.
#[tokio::test]
async fn highlight_nested_array_destructuring_lhs_is_write() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function foo(): void {
    [[$fi$0rst, $second], $third] = [[1, 2], 3];
//    ^^^^^^ write
    echo $first;
//       ^^^^^^ read
}
"#,
    )
    .await;
}
