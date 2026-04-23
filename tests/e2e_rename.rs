mod common;

use common::TestServer;

#[tokio::test]
async fn rename_function_produces_workspace_edit() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ren.php",
            "<?php\nfunction oldName(): void {}\noldName();\n",
        )
        .await;

    let resp = server.rename("ren.php", 1, 9, "newName").await;

    assert!(resp["error"].is_null(), "rename error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected rename to produce a WorkspaceEdit, got null"
    );
    assert!(
        result.get("changes").is_some() || result.get("documentChanges").is_some(),
        "rename result should be a WorkspaceEdit: {:?}",
        result
    );
    let uri = server.uri("ren.php");
    let file_edits = result["changes"][&uri]
        .as_array()
        .expect("expected edits for ren.php");
    assert_eq!(
        file_edits.len(),
        2,
        "expected 2 edits (declaration + call), got: {:?}",
        file_edits
    );
    let edited_lines: Vec<u64> = file_edits
        .iter()
        .map(|e| e["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert!(
        edited_lines.contains(&1),
        "declaration on line 1 must be renamed"
    );
    assert!(
        edited_lines.contains(&2),
        "call site on line 2 must be renamed"
    );
}

#[tokio::test]
async fn rename_variable_inside_enum_method() {
    let mut server = TestServer::new().await;
    server
        .open(
            "enum_ren.php",
            "<?php\nenum Status {\n    public function label($arg) { return $arg + 1; }\n}\n",
        )
        .await;

    let resp = server.rename("enum_ren.php", 2, 27, "value").await;

    assert!(resp["error"].is_null(), "rename error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected rename to produce a WorkspaceEdit, got null"
    );
    let uri = server.uri("enum_ren.php");
    let file_edits = result["changes"][&uri]
        .as_array()
        .expect("expected edits for enum_ren.php");
    assert_eq!(
        file_edits.len(),
        2,
        "expected 2 edits (param + body ref in enum method), got: {:?}",
        file_edits
    );
    let edited_lines: Vec<u64> = file_edits
        .iter()
        .map(|e| e["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert!(
        edited_lines.iter().all(|&l| l == 2),
        "both edits must be on line 2 (inside enum method), got: {:?}",
        edited_lines
    );
}
