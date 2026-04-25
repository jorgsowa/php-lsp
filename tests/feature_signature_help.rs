mod common;

use common::TestServer;
use expect_test::expect;

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
    expect!["▶ greet(string $name, int $count = 1)  @param0"].assert_eq(&out);
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
    expect!["▶ greet(string $name, int $count = 1)  @param1"].assert_eq(&out);
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
    expect!["▶ hello(string $name)  @param0"].assert_eq(&out);
}
