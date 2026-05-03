mod common;

use common::TestServer;
use expect_test::expect;

// ── basic shape: declaration only ───────────────────────────────────────────

#[tokio::test]
async fn class_with_only_declaration_yields_one_range() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nclass Lin$0kedClass {}\n")
        .await;
    expect![[r#"
        1:6-1:17
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

// ── functions ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn function_decl_links_to_all_call_sites() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range(
            r#"<?php
function gre$0et() {}
greet();
greet();
"#,
        )
        .await;
    expect![[r#"
        1:9-1:14
        2:0-2:5
        3:0-3:5
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn function_call_links_back_to_decl() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range(
            r#"<?php
function greet() {}
gr$0eet();
"#,
        )
        .await;
    expect![[r#"
        1:9-1:14
        2:0-2:5
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

// ── classes & members ──────────────────────────────────────────────────────

#[tokio::test]
async fn class_decl_and_new_expression() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nclass F$0oo {}\n$x = new Foo();\n")
        .await;
    expect![[r#"
        1:6-1:9
        2:9-2:12
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn method_decl_and_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range(
            r#"<?php
class Calc {
    public function ad$0d(): void {}
}
$c = new Calc();
$c->add();
"#,
        )
        .await;
    expect![[r#"
        2:20-2:23
        5:4-5:7
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

// ── variables ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn variable_in_scope_links_all_occurrences_with_dollar_pattern() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range(
            r#"<?php
function f(): void {
    $fo$0o = 1;
    echo $foo;
    $foo += 2;
}
"#,
        )
        .await;
    expect![[r#"
        2:4-2:8
        3:9-3:13
        4:4-4:8
        pattern: \$[a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn variable_does_not_cross_function_scope() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range(
            r#"<?php
function f() { $x$0 = 1; }
function g() { $x = 2; }
"#,
        )
        .await;
    expect![[r#"
        1:15-1:17
        pattern: \$[a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn cursor_on_dollar_sign_still_finds_variable() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nfunction f() { $0$x = 1; echo $x; }\n")
        .await;
    expect![[r#"
        1:15-1:17
        1:28-1:30
        pattern: \$[a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

// ── positions that should NOT trigger linked editing ────────────────────────

#[tokio::test]
async fn whitespace_returns_no_linked_editing() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nclass Foo {} $0  $x = 1;\n")
        .await;
    expect!["<no linked editing>"].assert_eq(&out);
}

#[tokio::test]
async fn unknown_word_returns_no_linked_editing() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\necho 'nob$0ody';\n")
        .await;
    expect!["<no linked editing>"].assert_eq(&out);
}

#[tokio::test]
async fn comment_word_matching_class_name_does_not_link() {
    // Bug-fix regression: word_at extracts `Foo` from the line comment,
    // document_highlights would find AST refs to `Foo`, but the cursor
    // sits in a comment that isn't itself an AST node — entering linked
    // mode would silently mirror typing into the comment over the real
    // class declaration and `new Foo()` call. The cursor-on-highlight
    // guard suppresses linked editing here.
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\n// uses Fo$0o here\nclass Foo {}\n$x = new Foo();\n")
        .await;
    expect!["<no linked editing>"].assert_eq(&out);
}

#[tokio::test]
async fn string_literal_word_matching_function_name_does_not_link() {
    // Same bug class as the comment case: cursor sits inside the literal
    // `'greet'` (not an identifier reference); linked editing would
    // otherwise mirror typing into the string over real call sites.
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nfunction greet() {}\n$x = 'gr$0eet';\ngreet();\n")
        .await;
    expect!["<no linked editing>"].assert_eq(&out);
}

// ── word pattern correctness ────────────────────────────────────────────────

#[tokio::test]
async fn non_variable_pattern_disallows_dollar_sign() {
    // The pattern returned for a class name must NOT permit `$`, otherwise
    // the LSP client could accept linked-mode typing of `$NewName` and
    // produce invalid PHP.
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nclass Fo$0o {}\n")
        .await;
    let pattern = out
        .lines()
        .find_map(|l| l.strip_prefix("pattern: "))
        .expect("response should include a wordPattern");
    assert!(
        !pattern.contains('$') || pattern.contains(r"\$"),
        "non-variable pattern must not allow leading $; got {pattern:?}"
    );
}

#[tokio::test]
async fn variable_pattern_requires_dollar_sign() {
    // The pattern returned for a variable must REQUIRE `\$`, otherwise the
    // user could type a name without `$` and break the variable.
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nfunction f() { $x$0 = 1; }\n")
        .await;
    let pattern = out
        .lines()
        .find_map(|l| l.strip_prefix("pattern: "))
        .expect("response should include a wordPattern");
    assert!(
        pattern.starts_with(r"\$"),
        "variable pattern must require leading \\$; got {pattern:?}"
    );
}

// ── unicode identifier support ─────────────────────────────────────────────

#[tokio::test]
async fn method_in_one_class_does_not_link_unrelated_class_with_same_name() {
    // Regression: two classes share a method name. Cursor on `bar` inside
    // class A must NOT link to `bar` inside class B — typing in linked
    // mode would otherwise corrupt B's method.
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range(
            r#"<?php
class A {
    public function ba$0r(): void {}
}
class B {
    public function bar(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        2:20-2:23
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn class_name_itself_still_links_globally() {
    // Cursor on the class header — the rename target IS the class. The
    // class-scope filter must NOT apply (otherwise the `new Foo()` site
    // gets dropped).
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nclass Fo$0o {}\n$x = new Foo();\n")
        .await;
    expect![[r#"
        1:6-1:9
        2:9-2:12
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn cjk_identifier_links_correctly() {
    // Regression for the BMP word-pattern range: identifiers using
    // characters beyond Latin-1 (e.g. CJK) must round-trip. The original
    // `\x80-\xff` byte range silently rejected anything past U+00FF.
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nfunction 名$0前() {}\n名前();\n")
        .await;
    expect![[r#"
        1:9-1:11
        2:0-2:2
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn utf8_identifier_links_correctly() {
    let mut s = TestServer::new().await;
    let out = s
        .check_linked_editing_range("<?php\nfunction caf$0é() {}\ncafé();\n")
        .await;
    expect![[r#"
        1:9-1:13
        2:0-2:4
        pattern: [a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*"#]]
    .assert_eq(&out);
}
