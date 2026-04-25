//! documentHighlight coverage using the `ref`/`read`/`write` annotation tags.

mod common;

use common::TestServer;
use serde_json::Value;

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

/// Function declaration and its two call sites — all three must be highlighted.
#[tokio::test]
async fn highlight_function_declaration_and_calls() {
    let mut s = TestServer::new().await;
    s.check_highlight_annotated(
        r#"<?php
function r$0un(): void {}
//       ^^^ ref
run();
// ^^^ ref
run();
// ^^^ ref
"#,
    )
    .await;
}

fn lines_of(hl: &[Value]) -> Vec<u64> {
    hl.iter()
        .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
        .collect()
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
