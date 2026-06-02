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

/// Highlights of a variable used as both param and body ref inside an enum
/// method — both occurrences are on the same line so we assert by count.
#[tokio::test]
async fn highlight_variable_inside_enum_method() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
enum Status {
    public function label($a$0rg) { return $arg + 1; }
}
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let highlights = resp["result"].as_array().expect("array");
    assert_eq!(
        highlights.len(),
        2,
        "expected 2 highlights (param + body ref): {highlights:?}"
    );
    let lines = lines_of(highlights);
    assert!(
        lines.iter().all(|&l| l == 2),
        "both highlights must be on the method body line: {lines:?}"
    );
}

/// Highlights must not bleed outer-scope variable with the same name into
/// an enum method's highlight set.
#[tokio::test]
async fn highlight_enum_method_does_not_bleed_outer_scope() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
$arg = 0;
enum Status {
    public function label($a$0rg) { return $arg + 1; }
}
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let highlights = resp["result"].as_array().expect("array");
    assert_eq!(
        highlights.len(),
        2,
        "expected exactly 2 highlights (param + body ref): {highlights:?}"
    );
    let lines = lines_of(highlights);
    assert!(
        lines.iter().all(|&l| l == 3),
        "outer $arg (line 1) must not appear: {lines:?}"
    );
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
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let result = &resp["result"];
    if let Some(highlights) = result.as_array() {
        assert_eq!(
            highlights.len(),
            0,
            "cursor on string literal should return no highlights"
        );
    } else {
        assert!(
            result.is_null(),
            "expected null or empty array for string literal, got: {result:?}"
        );
    }
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
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let highlights = resp["result"].as_array().expect("array");
    let out = highlights
        .iter()
        .map(|h| {
            let sl = h["range"]["start"]["line"].as_u64().unwrap_or(0);
            let sc = h["range"]["start"]["character"].as_u64().unwrap_or(0);
            let ec = h["range"]["end"]["character"].as_u64().unwrap_or(0);
            let kind = match h["kind"].as_u64() {
                Some(1) => "text",
                Some(2) => "read",
                Some(3) => "write",
                _ => "?",
            };
            format!("{sl}:{sc}-{ec} ({kind})")
        })
        .collect::<Vec<_>>()
        .join("\n");
    expect![[r#"
        1:9-14 (text)
        2:0-5 (text)"#]]
    .assert_eq(&out);
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
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let result = &resp["result"];
    if let Some(highlights) = result.as_array() {
        assert_eq!(
            highlights.len(),
            0,
            "cursor beyond line end should return no highlights"
        );
    } else {
        assert!(
            result.is_null(),
            "expected null or empty array when cursor is beyond line end, got: {result:?}"
        );
    }
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
    let opened = s
        .open_fixture(
            r#"<?php
function foo() {
    $x$0++;
    ++$x;
    --$x;
    $x--;
}
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let highlights = resp["result"].as_array().expect("array");
    assert_eq!(
        highlights.len(),
        4,
        "expected 4 highlights (all increment/decrement positions): {highlights:?}"
    );
}

#[tokio::test]
async fn highlight_foreach_key_binding_and_use() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
function foo($arr) {
    foreach ($arr as $k$0ey => $value) {
        echo $key;
    }
}
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let highlights = resp["result"].as_array().expect("array");
    assert_eq!(
        highlights.len(),
        2,
        "expected 2 highlights (binding + usage): {highlights:?}"
    );
}

#[tokio::test]
async fn highlight_foreach_value_binding_and_use() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
function foo($arr) {
    foreach ($arr as $v$0alue) {
        echo $value;
    }
}
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let highlights = resp["result"].as_array().expect("array");
    assert_eq!(
        highlights.len(),
        2,
        "expected 2 highlights (binding + usage): {highlights:?}"
    );
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
    let opened = s
        .open_fixture(
            r#"<?php
class Status {
    const AC$0TIVE = 1;
    const INACTIVE = 0;

    public function check() {
        if ($this->value === Status::ACTIVE) {
            return Status::ACTIVE;
        }
    }
}
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let highlights = resp["result"].as_array().expect("array");
    assert_eq!(
        highlights.len(),
        3,
        "expected 3 highlights (decl + 2 refs): {highlights:?}"
    );
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
    let highlights = resp["result"].as_array().expect("array");
    assert_eq!(
        highlights.len(),
        2,
        "should highlight decl + call, not the string"
    );
    let lines = lines_of(highlights);
    assert!(
        !lines.contains(&3),
        "line 3 (string literal) should not be highlighted: {lines:?}"
    );
}
