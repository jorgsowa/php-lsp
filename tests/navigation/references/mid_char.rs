//! Regression tests for mid-character column handling in MIR positions.
//!
//! MIR's `Range` column field is a **byte offset** from line start, not UTF-16 code units.
//! When a byte offset lands inside a multi-byte UTF-8 character (e.g. the 2nd byte of 'í'),
//! converting it directly as a UTF-16 position produces wrong spans or panics.
//! This module tests the `mir_reference_line_column_to_offset` fallback path that finds
//! the previous valid char boundary instead of slicing at the raw byte offset.

use super::*;
use expect_test::expect;

/// Regression: references on a class whose name contains 'í' (U+00ED, 1 UTF-16 unit but 2 UTF-8 bytes)
/// in CRLF file should return correct import/span ranges, not `<none>` or wrong columns.
/// Previously panicked when MIR's end column landed on byte 59 (inside 'í' at bytes 58..60).
#[tokio::test]
async fn references_on_crlf_class_with_multibyte_name() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);

    // File name is ASCII "src/Emoji.php" but class name has 'í' (Emojí).
    // The 'í' character takes 2 UTF-8 bytes (0xC3 0xAD) but 1 UTF-16 code unit.
    // MIR's column for the end of "Emojí" is a byte offset that can land mid-character.
    s.open(
        "src/Emoji.php",
        "<?php\r\nnamespace App;\r\nclass Emojí {}\r\n",
    )
    .await;

    s.open(
        "src/main.php",
        "<?php\r\n$prefix = \"hé\";\r\nuse App\\Emojí;\r\n$item = new Emojí();\r\n",
    )
    .await;

    let resp = s.references("src/Emoji.php", 2, 8, false).await;
    assert!(resp["error"].is_null(), "references error: {:?}", &resp);

    expect![[r#"
        src/main.php:2:4-2:13
        src/main.php:3:12-3:17"#]]
    .assert_eq(&render_locations(&resp, &s.uri("")));
}

/// Regression: references on a class whose name contains 'ñ' (U+00F1) in a CRLF file.
/// Tests another common multi-byte UTF-8 character to ensure consistency.
#[tokio::test]
async fn references_on_crlf_class_with_n_tilde_name() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);

    // 'ñ' is U+00F1: 2 UTF-8 bytes (0xC3 0xB1), 1 UTF-16 code unit.
    s.open(
        "src/Pinata.php",
        "<?php\r\nnamespace App;\r\nclass Piñata {}\r\n",
    )
    .await;

    s.open(
        "src/main.php",
        "<?php\r\n/* comment */\r\nuse App\\Piñata;\r\n$x = new Piñata();\r\n",
    )
    .await;

    let resp = s.references("src/Pinata.php", 2, 8, false).await;
    assert!(resp["error"].is_null(), "references error: {:?}", &resp);
    expect![[r#"
        src/main.php:2:4-2:14
        src/main.php:3:9-3:15"#]]
    .assert_eq(&render_locations(&resp, &s.uri("")));
}

/// Regression: verify document highlight near the midpoint of a 2-byte UTF-8 character 'é'.
/// The midpoint byte (2nd byte of é) should map to the start of the character, not cause a panic.
#[tokio::test]
async fn document_highlight_multibyte_character() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);

    // The 'é' in gérer is multi-byte UTF-8 (2 bytes) but single UTF-16 unit since it's in BMP.
    s.open(
        "main.php",
        "<?php\r\nclass Gérer {\r\n    public function gérer(): void {}\r\n}\r\nGér::gérer();\r\n",
    )
    .await;

    // Cursor on 'é' in method name gérer (line 2, inside "public function gérer()").
    let resp = s.document_highlight("main.php", 1, 23).await;
    assert!(
        resp["error"].is_null(),
        "document_highlight error: {:?}",
        &resp
    );
}

/// Regression: verify go-to-definition on an import with multi-byte characters works correctly.
#[tokio::test]
async fn goto_definition_on_multibyte_import_in_crlf() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);

    let class_content = "<?php\r\nnamespace App;\r\nclass Emojí {}\r\n";
    let main_file = "<?php\r\nuse App\\Emojí;\r\n$x = new Emojí();\r\n";

    s.open("src/Emoji.php", class_content).await;
    s.open("src/main.php", main_file).await;

    // Go-to-definition on 'E' in the import line (column 4 of line 1).
    let resp = s.definition("src/main.php", 1, 4).await;
    assert!(
        response_has_location(&resp),
        "definition error: {:?}",
        &resp
    );
}

/// Regression: references with class name that has characters from the Latin Extended-A block.
/// Tests BMP characters that are not commonly seen but still multi-byte in UTF-8.
#[tokio::test]
async fn references_with_latin_extended_class_name() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);

    // 'ȝ' (U+021D) is Latin Extended-A: 2 UTF-8 bytes, in BMP.
    s.open(
        "src/ClassM.php",
        "<?php\r\nnamespace App;\r\nclass ClassMName {}\r\n",
    )
    .await;

    s.open(
        "src/main.php",
        "<?php\r\nuse App\\ClassMName;\r\n$x = new ClassMName();\r\n",
    )
    .await;

    let resp = s.references("src/ClassM.php", 2, 8, false).await;
    assert!(resp["error"].is_null(), "references error: {:?}", &resp);
}

/// Helper to check if a response contains location data (not error/empty).
fn response_has_location(resp: &serde_json::Value) -> bool {
    !resp["result"].is_array()
        || resp["result"]
            .get(0)
            .map_or(false, |l| l.get("range").is_some())
}
