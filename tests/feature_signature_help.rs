mod common;

use common::TestServer;

#[tokio::test]
async fn signature_help_at_first_arg() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
function greet(string $name, int $count = 1): string { return $name; }
greet($0);
"#,
        )
        .await;
    assert!(out.contains("greet"), "expected signature: {out}");
    assert!(out.contains("@param0"), "expected @param0 active: {out}");
}

#[tokio::test]
async fn signature_help_at_second_arg() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
function greet(string $name, int $count = 1): string { return $name; }
greet('x', $0);
"#,
        )
        .await;
    assert!(out.contains("@param1"), "expected @param1 active: {out}");
}

#[tokio::test]
async fn signature_help_for_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_signature_help(
            r#"<?php
class Greeter {
    public function hello(string $name): string { return $name; }
}
$g = new Greeter();
$g->hello($0);
"#,
        )
        .await;
    assert!(out.contains("hello"), "expected method sig: {out}");
}
