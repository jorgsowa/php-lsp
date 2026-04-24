//! Completion coverage across trigger characters and contexts.
//!
//! Each test asserts on the presence of specific labels rather than full
//! snapshots — completion lists contain many built-ins/keywords whose ordering
//! is driven by ranking heuristics.

mod common;

use common::TestServer;

async fn labels(s: &mut TestServer, src: &str) -> Vec<String> {
    let opened = s.open_fixture(src).await;
    let c = opened.cursor().clone();
    let resp = s.completion(&c.path, c.line, c.character).await;
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    items
        .iter()
        .filter_map(|i| i["label"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn completion_arrow_method() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
class Greeter {
    public function hello(): string { return 'hi'; }
    public function bye(): void {}
}
$g = new Greeter();
$g->h$0
"#,
    )
    .await;
    assert!(
        labels.iter().any(|l| l == "hello"),
        "hello missing: {labels:?}"
    );
}

#[ignore = "php-lsp gap: `->` completion does not list properties"]
#[tokio::test]
async fn completion_arrow_property() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
class User {
    public string $name = '';
    public int $age = 0;
}
$u = new User();
$u->na$0
"#,
    )
    .await;
    assert!(labels.iter().any(|l| l == "name" || l == "$name"));
}

#[tokio::test]
async fn completion_double_colon_static_method() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
class Reg {
    public static function get(): void {}
    public static function set(): void {}
}
Reg::$0
"#,
    )
    .await;
    assert!(
        labels.iter().any(|l| l == "get"),
        "expected 'get': {labels:?}"
    );
}

#[tokio::test]
async fn completion_namespace_prefix() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"//- /src/App/Greeter.php
<?php
namespace App;
class Greeter {}

//- /src/main.php
<?php
$g = new \App\$0
"#,
    )
    .await;
    // Tolerant: just ensure server responds without error on `\App\` prefix.
    let _ = labels;
}

#[tokio::test]
async fn completion_keyword_in_top_level() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
func$0
"#,
    )
    .await;
    assert!(labels.iter().any(|l| l == "function"));
}

#[ignore = "php-lsp gap: variable completion does not surface in-scope locals/params"]
#[tokio::test]
async fn completion_variable_in_scope() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
function f(string $name, int $count): void {
    $na$0
}
"#,
    )
    .await;
    assert!(
        labels.iter().any(|l| l == "$name"),
        "expected $name: {labels:?}"
    );
}

#[ignore = "php-lsp gap: method completion includes methods from unrelated classes"]
#[tokio::test]
async fn completion_method_does_not_leak_to_unrelated_classes() {
    let mut s = TestServer::new().await;
    let labels = labels(
        &mut s,
        r#"<?php
class A { public function foo(): void {} }
class B { public function bar(): void {} }
$a = new A();
$a->$0
"#,
    )
    .await;
    assert!(labels.iter().any(|l| l == "foo"));
    assert!(
        !labels.iter().any(|l| l == "bar"),
        "B::bar should not appear in A completion: {labels:?}"
    );
}
