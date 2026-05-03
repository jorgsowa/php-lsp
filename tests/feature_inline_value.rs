mod common;

use common::TestServer;
use expect_test::expect;

// ── basic shape ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn returns_lookup_with_full_payload_for_each_variable() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inline_value("<?php\n$0$x = 42;\n$y = $x + 1;$0\n")
        .await;
    expect![[r#"
        1:0-1:2 $x (case-sensitive)
        2:0-2:2 $y (case-sensitive)
        2:5-2:7 $x (case-sensitive)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn empty_range_yields_no_values() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inline_value("<?php\nfunction f(): void {\n    // no vars\n}\n")
        .await;
    expect!["<no inline values>"].assert_eq(&out);
}

// ── range filtering ─────────────────────────────────────────────────────────

#[tokio::test]
async fn excludes_lines_outside_range() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inline_value("<?php\n$x = 1;\n$0$y = 2;$0\n$z = 3;\n")
        .await;
    expect!["2:0-2:2 $y (case-sensitive)"].assert_eq(&out);
}

#[tokio::test]
async fn covers_full_multiline_range() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inline_value("<?php\n$0$a = 1;\n$b = 2;\n$c = 3;$0\n$d = 4;\n")
        .await;
    expect![[r#"
        1:0-1:2 $a (case-sensitive)
        2:0-2:2 $b (case-sensitive)
        3:0-3:2 $c (case-sensitive)"#]]
    .assert_eq(&out);
}

// ── identifier shapes ───────────────────────────────────────────────────────

#[tokio::test]
async fn names_with_underscores_and_digits() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inline_value("<?php\n$0$_first = 1; $second_2 = 2;$0\n")
        .await;
    expect![[r#"
        1:0-1:7 $_first (case-sensitive)
        1:13-1:22 $second_2 (case-sensitive)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn name_starting_with_digit_is_not_a_variable() {
    // `$0`, `$1` etc. are not valid PHP variable names; the scanner rejects
    // anything whose first identifier byte isn't alpha/underscore/UTF-8.
    let mut s = TestServer::new().await;
    let out = s.check_inline_value("<?php\n$0echo 'no vars';$0\n").await;
    expect!["<no inline values>"].assert_eq(&out);
}

#[tokio::test]
async fn skips_this_in_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inline_value("<?php\n$0$this->foo = $bar;$0\n")
        .await;
    expect!["1:13-1:17 $bar (case-sensitive)"].assert_eq(&out);
}

#[tokio::test]
async fn skips_variable_variables() {
    let mut s = TestServer::new().await;
    let out = s.check_inline_value("<?php\n$0$$dynamic = 1;$0\n").await;
    expect!["<no inline values>"].assert_eq(&out);
}

#[tokio::test]
async fn lone_dollar_without_identifier_is_skipped() {
    // `$` followed by a non-identifier char (e.g. whitespace, operator) is
    // not a variable. Make sure the scanner doesn't emit a zero-length lookup.
    let mut s = TestServer::new().await;
    let out = s.check_inline_value("<?php\n$0$ = 1;$0\n").await;
    expect!["<no inline values>"].assert_eq(&out);
}

// ── multiple references on one line ─────────────────────────────────────────

#[tokio::test]
async fn multiple_occurrences_on_same_line_each_get_a_lookup() {
    let mut s = TestServer::new().await;
    let out = s.check_inline_value("<?php\n$0$x = $y + $y;$0\n").await;
    expect![[r#"
        1:0-1:2 $x (case-sensitive)
        1:5-1:7 $y (case-sensitive)
        1:10-1:12 $y (case-sensitive)"#]]
    .assert_eq(&out);
}

// ── current-behavior snapshots: no lexer awareness ──────────────────────────
// `inline_values_in_range` is a byte-level scanner with no understanding of
// strings or comments; PHP variables that appear inside string literals or
// comments are reported as if they were live references. Pinned so any
// future lexer-aware rewrite shows up as a snapshot diff.

#[tokio::test]
async fn variable_inside_double_quoted_string_is_reported() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inline_value("<?php\n$0echo \"hello $name\";$0\n")
        .await;
    expect!["1:12-1:17 $name (case-sensitive)"].assert_eq(&out);
}

#[tokio::test]
async fn variable_inside_single_quoted_string_is_reported() {
    // PHP does NOT interpolate single-quoted strings — `'$name'` is just the
    // literal six characters. The scanner doesn't know that and emits a
    // lookup anyway. Pinned as a snapshot.
    let mut s = TestServer::new().await;
    let out = s.check_inline_value("<?php\n$0$s = '$name';$0\n").await;
    expect![[r#"
        1:0-1:2 $s (case-sensitive)
        1:6-1:11 $name (case-sensitive)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn variable_inside_line_comment_is_reported() {
    let mut s = TestServer::new().await;
    let out = s.check_inline_value("<?php\n$0// look at $foo$0\n").await;
    expect!["1:11-1:15 $foo (case-sensitive)"].assert_eq(&out);
}

#[tokio::test]
async fn case_sensitive_lookup_is_always_true() {
    // PHP variable lookup IS case-sensitive (unlike PHP function names),
    // so the field is hard-coded to `true` and that should never regress.
    let mut s = TestServer::new().await;
    let out = s.check_inline_value("<?php\n$0$Foo = 1;$0\n").await;
    expect!["1:0-1:4 $Foo (case-sensitive)"].assert_eq(&out);
}
