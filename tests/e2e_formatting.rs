mod common;

use common::TestServer;

#[tokio::test]
async fn formatting_returns_null_or_valid_edits() {
    let mut server = TestServer::new().await;
    server
        .open("fmt.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.formatting("fmt.php").await;

    assert!(resp["error"].is_null(), "formatting error: {:?}", resp);
    // null = no formatter installed on this machine; array = formatter produced edits
    match resp["result"].as_array() {
        None => assert!(
            resp["result"].is_null(),
            "expected null (no formatter) or TextEdit array, got: {:?}",
            resp["result"]
        ),
        Some(edits) => {
            assert!(!edits.is_empty(), "formatter returned empty edit array");
            for edit in edits {
                assert!(
                    edit["range"].is_object(),
                    "TextEdit missing 'range': {:?}",
                    edit
                );
                assert!(
                    edit["newText"].is_string(),
                    "TextEdit missing 'newText': {:?}",
                    edit
                );
            }
        }
    }
}

#[tokio::test]
async fn range_formatting_returns_null_or_valid_edits() {
    let mut server = TestServer::new().await;
    server
        .open("rfmt.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.range_formatting("rfmt.php", 0, 0, 2, 0).await;

    assert!(resp["error"].is_null(), "rangeFormatting error: {:?}", resp);
    match resp["result"].as_array() {
        None => assert!(
            resp["result"].is_null(),
            "expected null (no formatter) or TextEdit array, got: {:?}",
            resp["result"]
        ),
        Some(edits) => {
            assert!(!edits.is_empty(), "formatter returned empty edit array");
            for edit in edits {
                assert!(
                    edit["range"].is_object(),
                    "TextEdit missing 'range': {:?}",
                    edit
                );
                assert!(
                    edit["newText"].is_string(),
                    "TextEdit missing 'newText': {:?}",
                    edit
                );
            }
        }
    }
}

/// Unknown trigger characters must return null — the handler only supports `}` and `\n`.
#[tokio::test]
async fn on_type_formatting_unknown_trigger_returns_null() {
    let mut server = TestServer::new().await;
    server.open("otfmt.php", "<?php\nif (true) {\n").await;

    let resp = server.on_type_formatting("otfmt.php", 1, 10, "{").await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    assert!(
        resp["result"].is_null(),
        "expected null for unhandled trigger '{{', got: {:?}",
        resp["result"]
    );
}

/// The `}` trigger is handled in-process (no external tool needed) and must
/// de-indent the closing brace to match the indentation of the opening `{`.
#[tokio::test]
async fn on_type_formatting_close_brace_deindents() {
    let mut server = TestServer::new().await;
    // Line 2 has "    }" (4-space indent); the matching `{` on line 1 has 0 indent.
    server
        .open("otfmt2.php", "<?php\nif (true) {\n    }\n")
        .await;

    // Cursor is at line 2, character 4 (just after the indent, on `}`).
    let resp = server.on_type_formatting("otfmt2.php", 2, 4, "}").await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    let edits = resp["result"]
        .as_array()
        .expect("} trigger must produce a TextEdit array");
    assert_eq!(edits.len(), 1, "expected exactly one de-indent edit");

    let edit = &edits[0];
    assert_eq!(
        edit["range"]["start"],
        serde_json::json!({"line": 2, "character": 0}),
        "edit start must be at line 2, character 0"
    );
    assert_eq!(
        edit["range"]["end"],
        serde_json::json!({"line": 2, "character": 4}),
        "edit end must be at line 2, character 4 (replacing 4-space indent)"
    );
    assert_eq!(
        edit["newText"].as_str().unwrap(),
        "",
        "newText must be empty (de-indent to column 0)"
    );
}
