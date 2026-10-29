//! documentHighlight coverage using the `ref`/`read`/`write` annotation tags.

use super::*;
use expect_test::expect;

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

/// `final` (a semi-reserved word) is a valid method name. Before the
/// bare-keyword gate in `document_highlights`, the class-modifier `final`
/// here matched by bare name via `refs_in_stmts`, highlighting the
/// unrelated method's declaration and call site instead of nothing.
#[tokio::test]
async fn highlight_class_modifier_keyword_ignores_same_file_name_collision() {
    let mut s = TestServer::new().await;
    let opened = s
        .open_fixture(
            r#"<?php
class Registry {
    public function final(): void {}
}
$r = new Registry();
$r->final();
fina$0l class Locked {}
"#,
        )
        .await;
    let c = opened.cursor();
    let resp = s.document_highlight(&c.path, c.line, c.character).await;
    expect!["<no highlights>"].assert_eq(&render_document_highlight(&resp));
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
