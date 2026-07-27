//! Inline variable code action transformation tests.
//! Tests verify that variables are replaced with their assigned expressions.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn inline_variable_simple_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$greeting = "Hello";
echo $0$greeting$0;
"#,
            "Inline variable '$greeting'",
        )
        .await;
    expect![[r#"
        <?php
        echo "Hello";
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_numeric_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$count = 42;
$total = $0$count$0 * 2;
"#,
            "Inline variable '$count'",
        )
        .await;
    expect![[r#"
        <?php
        $total = 42 * 2;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_removes_assignment() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = 5;
echo $0$x$0;
echo $x;
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        echo 5;
        echo 5;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_function_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$name = getName();
echo "Hello " . $0$name$0;
"#,
            "Inline variable '$name'",
        )
        .await;
    expect![[r#"
        <?php
        echo "Hello " . getName();
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_array_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$items = [1, 2, 3];
foreach ($0$items$0 as $item) {
    echo $item;
}
"#,
            "Inline variable '$items'",
        )
        .await;
    expect![[r#"
        <?php
        foreach ([1, 2, 3] as $item) {
            echo $item;
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_with_complex_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$result = $this->getValue() + 10 * 2;
echo $0$result$0;
"#,
            "Inline variable '$result'",
        )
        .await;
    expect![[r#"
        <?php
        echo $this->getValue() + 10 * 2;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_multiple_uses_same_line() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = 5;
echo $0$x$0 + $x;
"#,
            "Inline variable '$x'",
        )
        .await;
    // Both uses should be replaced
    expect![[r#"
        <?php
        echo 5 + 5;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_no_action_when_multiple_assignments() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$tmp = 1;
$tmp = 2;
return $0$tmp$0;
"#,
            "Inline variable '$tmp'",
        )
        .await;
    expect!["<action not found: Inline variable '$tmp'>"].assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_no_action_when_later_assignment_exists() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$tmp = 1;
echo $0$tmp$0;
$tmp = 2;
"#,
            "Inline variable '$tmp'",
        )
        .await;
    expect!["<action not found: Inline variable '$tmp'>"].assert_eq(&out);
}

#[tokio::test]
async fn inline_variable_equality_does_not_block_inline() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = 1;
if ($x == 2) {
    echo $0$x$0;
}
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        if (1 == 2) {
            echo 1;
        }
    "#]]
    .assert_eq(&out);
}

/// A second statement on the same line as the assignment (`$x = foo();
/// doSomethingElse();`) must not be swallowed into the RHS — deleting the
/// whole line to inline would silently destroy `doSomethingElse()`, so the
/// action must refuse rather than guess.
#[tokio::test]
async fn inline_variable_no_action_when_second_statement_on_same_line() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = foo(); doSomethingElse();
echo $0$x$0;
"#,
            "Inline variable '$x'",
        )
        .await;
    expect!["<action not found: Inline variable '$x'>"].assert_eq(&out);
}

/// A semicolon inside a nested call's string argument must not be mistaken
/// for the statement terminator when extracting the RHS.
#[tokio::test]
async fn inline_variable_rhs_with_semicolon_in_string_argument() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = foo("a;b");
echo $0$x$0;
"#,
            "Inline variable '$x'",
        )
        .await;
    expect![[r#"
        <?php
        echo foo("a;b");
    "#]]
    .assert_eq(&out);
}
