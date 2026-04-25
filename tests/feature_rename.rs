//! Rename coverage: prepareRename bounds + actual rename across files.

mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn prepare_rename_on_identifier() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_rename(
            r#"<?php
function gre$0et(): void {}
"#,
        )
        .await;
    expect!["1:9-1:14"].assert_eq(&out);
}

#[tokio::test]
async fn rename_function_same_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
function gre$0et(): void {}
greet();
greet();
"#,
            "salute",
        )
        .await;
    expect![[r#"
        // main.php
        1:9-1:14 → "salute"
        2:0-2:5 → "salute"
        3:0-3:5 → "salute""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn rename_method_across_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
}
$g = new Greeter();
$g->hello();
"#,
            "salute",
        )
        .await;
    expect![[r#"
        // main.php
        2:20-2:25 → "salute"
        5:4-5:9 → "salute""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn rename_class_updates_new_sites() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
class Wid$0get {}
$a = new Widget();
$b = new Widget();
"#,
            "Gadget",
        )
        .await;
    expect![[r#"
        // main.php
        1:6-1:12 → "Gadget"
        2:9-2:15 → "Gadget"
        3:9-3:15 → "Gadget""#]]
    .assert_eq(&out);
}
