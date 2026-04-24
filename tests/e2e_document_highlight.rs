mod common;

use common::TestServer;
use serde_json::Value;

fn lines_of(hl: &[Value]) -> Vec<u64> {
    hl.iter()
        .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
        .collect()
}

#[tokio::test]
async fn document_highlight_marks_occurrences() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function r$0un(): void {}
run();
run();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server
        .document_highlight(&c.path, c.line, c.character)
        .await;
    assert!(resp["error"].is_null(), "documentHighlight error: {resp:?}");
    let highlights = resp["result"].as_array().expect("array");
    assert_eq!(
        highlights.len(),
        3,
        "expected 3 highlights (1 decl + 2 calls): {highlights:?}"
    );
    let lines = lines_of(highlights);
    assert!(lines.contains(&1), "decl highlight missing on line 1");
    assert!(lines.contains(&2), "call highlight missing on line 2");
    assert!(lines.contains(&3), "call highlight missing on line 3");
}

#[tokio::test]
async fn document_highlight_variable_inside_enum_method() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
enum Status {
    public function label($a$0rg) { return $arg + 1; }
}
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server
        .document_highlight(&c.path, c.line, c.character)
        .await;
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

#[tokio::test]
async fn document_highlight_enum_method_does_not_bleed_outer_scope() {
    let mut server = TestServer::new().await;
    let opened = server
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

    let resp = server
        .document_highlight(&c.path, c.line, c.character)
        .await;
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
