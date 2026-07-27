//! Text formatting: document formatting, range formatting, and on-type formatting.

use super::*;

use expect_test::expect;

/// `formatting`/`rangeFormatting` delegate to `php-cs-fixer`/`phpcbf` on
/// `$PATH` (see `src/editing/formatting.rs`), and CI sets `tools: none` for
/// its PHP setup, so neither is ever installed — these handlers always take
/// the "no formatter" path in every environment this suite runs in.
#[tokio::test]
async fn formatting_returns_null_without_external_formatter() {
    let mut server = TestServer::new().await;
    server
        .open("fmt.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.formatting("fmt.php").await;

    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

#[tokio::test]
async fn range_formatting_returns_null_without_external_formatter() {
    let mut server = TestServer::new().await;
    server
        .open("rfmt.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.range_formatting("rfmt.php", 0, 0, 2, 0).await;

    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
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
    server
        .open("otfmt2.php", "<?php\nif (true) {\n    }\n")
        .await;

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

/// Close brace in nested block must align to the inner `if` indent.
#[tokio::test]
async fn on_type_formatting_close_brace_nested_block() {
    let mut server = TestServer::new().await;
    server
        .open(
            "otfmt_nested.php",
            "<?php\nif (true) {\n    if (false) {\n        }\n}\n",
        )
        .await;

    let resp = server
        .on_type_formatting("otfmt_nested.php", 3, 8, "}")
        .await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    let edits = resp["result"]
        .as_array()
        .expect("} trigger must produce a TextEdit array");
    assert_eq!(edits.len(), 1, "expected exactly one edit for nested block");

    let edit = &edits[0];
    assert_eq!(
        edit["range"]["start"],
        serde_json::json!({"line": 3, "character": 0}),
        "edit must start at column 0"
    );
    assert_eq!(
        edit["range"]["end"],
        serde_json::json!({"line": 3, "character": 8}),
        "edit must replace 8-space indent"
    );
    assert_eq!(
        edit["newText"].as_str().unwrap(),
        "    ",
        "newText must be 4-space indent (matching inner if)"
    );
}

/// Close brace already at correct indent produces no edits.
#[tokio::test]
async fn on_type_formatting_close_brace_already_aligned() {
    let mut server = TestServer::new().await;
    server
        .open("otfmt_aligned.php", "<?php\nif (true) {\n}\n")
        .await;

    let resp = server
        .on_type_formatting("otfmt_aligned.php", 2, 0, "}")
        .await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        result.is_null(),
        "no edit needed when already aligned; expected null, got: {:?}",
        result
    );
}

/// A stray `}` inside a string literal earlier in the block must not throw
/// off the brace-depth scan used to find the matching `{`. Nested one level
/// deeper than the string so a miscounted depth would match the *outer*
/// class's brace (column 0) instead of the inner method's (column 4) —
/// distinguishing a real bug from the "no match found" fallback, which also
/// happens to be column 0.
#[tokio::test]
async fn on_type_formatting_close_brace_ignores_brace_in_string() {
    let mut server = TestServer::new().await;
    server
        .open(
            "otfmt_string_brace.php",
            "<?php\nclass Foo {\n    public function bar() {\n        $x = \"}\";\n        }\n}\n",
        )
        .await;

    let resp = server
        .on_type_formatting("otfmt_string_brace.php", 4, 9, "}")
        .await;

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
        serde_json::json!({"line": 4, "character": 0}),
    );
    assert_eq!(
        edit["range"]["end"],
        serde_json::json!({"line": 4, "character": 8}),
    );
    assert_eq!(
        edit["newText"].as_str().unwrap(),
        "    ",
        "must de-indent to match the `function bar()` opening brace at column 4, \
         not be thrown off by the closing brace inside the string literal above \
         into matching the outer `class Foo` brace instead"
    );
}

/// Range formatting with a single-line range.
///
/// No formatter is installed in this suite's environment (see
/// `formatting_returns_null_without_external_formatter`), so this
/// deterministically returns null regardless of the requested range.
#[tokio::test]
async fn range_formatting_single_line_range() {
    let mut server = TestServer::new().await;
    server
        .open(
            "rfmt_single.php",
            "<?php\nfunction ugly( $x ){return $x;}\n",
        )
        .await;

    let resp = server
        .range_formatting("rfmt_single.php", 1, 0, 1, 38)
        .await;

    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

/// Range formatting covering the entire file.
///
/// No formatter is installed in this suite's environment (see
/// `formatting_returns_null_without_external_formatter`), so this
/// deterministically returns null regardless of the requested range.
#[tokio::test]
async fn range_formatting_entire_file_range() {
    let mut server = TestServer::new().await;
    server
        .open("rfmt_all.php", "<?php\nfunction ugly( $x ){return $x;}\n\n")
        .await;

    let resp = server.range_formatting("rfmt_all.php", 0, 0, 3, 0).await;

    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

/// Newline trigger after opening brace must indent the new line.
#[tokio::test]
async fn on_type_formatting_newline_indents_after_open_brace() {
    let mut server = TestServer::new().await;
    server
        .open("otfmt_nl1.php", "<?php\nif (true) {\n\n}")
        .await;

    let resp = server.on_type_formatting("otfmt_nl1.php", 2, 0, "\n").await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    let edits = resp["result"]
        .as_array()
        .expect("newline trigger must produce edits");
    assert_eq!(
        edits.len(),
        1,
        "expected exactly one indent edit for newline after brace"
    );
    assert_eq!(
        edits[0]["newText"].as_str().unwrap(),
        "    ",
        "newline should produce 4-space indent"
    );
}

/// Newline trigger on an indented line copies the base indent.
#[tokio::test]
async fn on_type_formatting_newline_copies_base_indent() {
    let mut server = TestServer::new().await;
    server.open("otfmt_nl2.php", "<?php\n    $x = 1;\n").await;

    let resp = server.on_type_formatting("otfmt_nl2.php", 2, 0, "\n").await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    let edits = resp["result"]
        .as_array()
        .expect("newline trigger must produce edits");
    assert_eq!(
        edits.len(),
        1,
        "expected exactly one indent edit for newline"
    );
    assert_eq!(
        edits[0]["newText"].as_str().unwrap(),
        "    ",
        "newline should copy the 4-space base indent"
    );
}

/// Newline trigger with tab indentation (insertSpaces: false).
#[tokio::test]
async fn on_type_formatting_newline_uses_tabs() {
    let mut server = TestServer::new().await;
    server
        .open("otfmt_nl_tabs.php", "<?php\nif (true) {\n\n}")
        .await;

    let resp = server
        .on_type_formatting_with_options("otfmt_nl_tabs.php", 2, 0, "\n", 4, false)
        .await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    let edits = resp["result"]
        .as_array()
        .expect("newline trigger with tabs must produce edits");
    assert_eq!(edits.len(), 1, "expected exactly one edit");
    assert_eq!(
        edits[0]["newText"].as_str().unwrap(),
        "\t",
        "newline with insertSpaces=false should produce tab indent"
    );
}

/// Newline at top level (line 0) produces no edits.
#[tokio::test]
async fn on_type_formatting_newline_at_top_level_no_edit() {
    let mut server = TestServer::new().await;
    server.open("otfmt_nl_toplevel.php", "<?php\n").await;

    let resp = server
        .on_type_formatting("otfmt_nl_toplevel.php", 1, 0, "\n")
        .await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        result.is_null(),
        "newline at top level should produce no edit; got: {:?}",
        result
    );
}

/// Newline when the line already has correct indent produces no edits.
#[tokio::test]
async fn on_type_formatting_newline_no_edit_when_already_correct() {
    let mut server = TestServer::new().await;
    server
        .open("otfmt_nl_correct.php", "<?php\nif (true) {\n    ")
        .await;

    let resp = server
        .on_type_formatting("otfmt_nl_correct.php", 2, 4, "\n")
        .await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        result.is_null(),
        "no edit needed when indent is already correct; got: {:?}",
        result
    );
}

/// Range formatting on a snippet without a leading `<?php` tag.
///
/// No formatter is installed in this suite's environment (see
/// `formatting_returns_null_without_external_formatter`), so this
/// deterministically returns null regardless of the requested range.
#[tokio::test]
async fn range_formatting_non_php_tagged_snippet() {
    let mut server = TestServer::new().await;
    server
        .open(
            "rfmt_no_opener.php",
            "<?php\nif (true)\n{\n    echo 'hello';\n}\n",
        )
        .await;

    // Format lines 2-4 (the if body, without <?php header)
    let resp = server
        .range_formatting("rfmt_no_opener.php", 2, 0, 4, 0)
        .await;

    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

/// Range formatting must not produce edits outside the requested range.
///
/// No formatter is installed in this suite's environment (see
/// `formatting_returns_null_without_external_formatter`), so this
/// deterministically returns null regardless of the requested range.
#[tokio::test]
async fn range_formatting_returns_no_edits_outside_requested_range() {
    let mut server = TestServer::new().await;
    server
        .open(
            "rfmt_bounded.php",
            "<?php\nfunction ugly( $x ){return $x;}\nfunction pretty() { return 1; }\n",
        )
        .await;

    // Format only line 1 (the first function)
    let resp = server
        .range_formatting("rfmt_bounded.php", 1, 0, 1, 37)
        .await;

    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

/// `}` trigger on a line index beyond the end of the file must return null/no-edits.
#[tokio::test]
async fn on_type_formatting_cursor_beyond_file_end_returns_empty() {
    let mut server = TestServer::new().await;
    server.open("otfmt_oob.php", "<?php\n$x = 1;\n").await;

    // Line 99 does not exist; close_brace should return vec![] which maps to null.
    let resp = server.on_type_formatting("otfmt_oob.php", 99, 0, "}").await;

    assert!(
        resp["error"].is_null(),
        "onTypeFormatting error: {:?}",
        resp
    );
    assert!(
        resp["result"].is_null(),
        "expected null result for cursor beyond file end, got: {:?}",
        resp["result"]
    );
}
