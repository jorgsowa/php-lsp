mod common;

use common::TestServer;

#[tokio::test]
async fn rename_function_produces_workspace_edit() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function o$0ldName(): void {}
oldName();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.rename(&c.path, c.line, c.character, "newName").await;
    assert!(resp["error"].is_null(), "rename error: {resp:?}");
    let result = &resp["result"];
    assert!(!result.is_null(), "expected a WorkspaceEdit");
    assert!(
        result.get("changes").is_some() || result.get("documentChanges").is_some(),
        "rename result should be a WorkspaceEdit: {result:?}"
    );
    let uri = server.uri(&c.path);
    let file_edits = result["changes"][&uri]
        .as_array()
        .expect("expected edits for the opened file");
    assert_eq!(
        file_edits.len(),
        2,
        "expected 2 edits (declaration + call): {file_edits:?}"
    );
    let edited_lines: Vec<u64> = file_edits
        .iter()
        .map(|e| e["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert!(edited_lines.contains(&1), "declaration must be renamed");
    assert!(edited_lines.contains(&2), "call site must be renamed");
}

#[tokio::test]
async fn rename_variable_inside_enum_method() {
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

    let resp = server.rename(&c.path, c.line, c.character, "value").await;
    assert!(resp["error"].is_null(), "rename error: {resp:?}");
    let result = &resp["result"];
    assert!(!result.is_null(), "expected WorkspaceEdit");
    let uri = server.uri(&c.path);
    let file_edits = result["changes"][&uri]
        .as_array()
        .expect("expected edits for the opened file");
    assert_eq!(
        file_edits.len(),
        2,
        "expected 2 edits (param + body ref): {file_edits:?}"
    );
    let edited_lines: Vec<u64> = file_edits
        .iter()
        .map(|e| e["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert!(
        edited_lines.iter().all(|&l| l == 2),
        "both edits must be inside the enum method: {edited_lines:?}"
    );
}
