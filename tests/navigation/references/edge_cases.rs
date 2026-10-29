//! Edge case and regression tests: partial matching, kind filtering, class references, attributes.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn references_interface_method_excluded_with_include_declaration_false() {
    // With includeDeclaration=false the interface method declaration must not appear.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
interface I {
    public function a$0dd(): void;
}
$obj->add();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = s.references(&c.path, c.line, c.character, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    use expect_test::expect;
    expect!["main.php:4:6-4:9"].assert_eq(&render_locations(&resp, &s.uri("")));
}

/// mir reports code-point columns; an emoji before a call shifts its UTF-16 column by two.
#[tokio::test]
async fn references_on_emoji_line_use_utf16_columns() {
    let mut s = TestServer::new().await;
    s.open(
        "main.php",
        "<?php\nfunction bump(): void {}\necho '😀'; bump();\n",
    )
    .await;

    let resp = s.references("main.php", 1, 10, false).await;
    expect!["main.php:2:11-2:15"].assert_eq(&render_locations(&resp, &s.uri("")));

    let resp = s.references("main.php", 1, 10, true).await;
    expect![[r#"
        main.php:1:9-1:13
        main.php:2:11-2:15"#]]
    .assert_eq(&render_locations(&resp, &s.uri("")));
}
