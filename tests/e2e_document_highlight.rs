mod common;

use common::TestServer;

#[tokio::test]
async fn document_highlight_marks_occurrences() {
    let mut server = TestServer::new().await;
    server
        .open("hl.php", "<?php\nfunction run(): void {}\nrun();\nrun();\n")
        .await;

    let resp = server.document_highlight("hl.php", 1, 9).await;

    assert!(
        resp["error"].is_null(),
        "documentHighlight error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "documentHighlight must return an array: {:?}",
        result
    );
    let highlights = result.as_array().unwrap();
    assert_eq!(
        highlights.len(),
        3,
        "expected 3 highlights (1 declaration + 2 calls), got: {:?}",
        highlights
    );
    let lines: Vec<u64> = highlights
        .iter()
        .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert!(
        lines.contains(&1),
        "declaration highlight missing on line 1"
    );
    assert!(lines.contains(&2), "call highlight missing on line 2");
    assert!(lines.contains(&3), "call highlight missing on line 3");
}

#[tokio::test]
async fn document_highlight_variable_inside_enum_method() {
    let mut server = TestServer::new().await;
    server
        .open(
            "enum_hl.php",
            "<?php\nenum Status {\n    public function label($arg) { return $arg + 1; }\n}\n",
        )
        .await;

    let resp = server.document_highlight("enum_hl.php", 2, 27).await;

    assert!(
        resp["error"].is_null(),
        "documentHighlight error: {:?}",
        resp
    );
    let highlights = resp["result"].as_array().expect("expected array");
    assert_eq!(
        highlights.len(),
        2,
        "expected 2 highlights (param + body ref in enum method), got: {:?}",
        highlights
    );
    let lines: Vec<u64> = highlights
        .iter()
        .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert!(
        lines.iter().all(|&l| l == 2),
        "both highlights must be on line 2 (inside enum method), got: {:?}",
        lines
    );
}

#[tokio::test]
async fn document_highlight_enum_method_does_not_bleed_outer_scope() {
    let mut server = TestServer::new().await;
    server
        .open(
            "enum_scope.php",
            "<?php\n$arg = 0;\nenum Status {\n    public function label($arg) { return $arg + 1; }\n}\n",
        )
        .await;

    let resp = server.document_highlight("enum_scope.php", 3, 27).await;

    assert!(
        resp["error"].is_null(),
        "documentHighlight error: {:?}",
        resp
    );
    let highlights = resp["result"].as_array().expect("expected array");
    assert_eq!(
        highlights.len(),
        2,
        "expected exactly 2 highlights (param + body ref), got: {:?}",
        highlights
    );
    let lines: Vec<u64> = highlights
        .iter()
        .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert!(
        lines.iter().all(|&l| l == 3),
        "all highlights must be on line 3 (inside enum method), outer $arg must not appear: {:?}",
        lines
    );
}
