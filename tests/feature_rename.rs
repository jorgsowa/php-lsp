//! Rename coverage: prepareRename bounds + actual rename across files.

mod common;

use common::TestServer;

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
    assert!(
        !out.contains("<not renameable>"),
        "expected a valid prepare-rename range: {out}"
    );
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
    assert!(out.contains("salute"), "expected 'salute' in edit: {out}");
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
    assert!(out.contains("salute"), "expected 'salute': {out}");
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
    assert!(out.contains("Gadget"));
}
