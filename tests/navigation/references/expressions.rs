//! Expression-context tests: function/method calls in various control flow structures.

use super::*;

#[tokio::test]
async fn references_nested_function_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function gr$0eet(): void {}
//       ^^^^^ def
echo(greet());
//   ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_function_call_inside_if_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function che$0ck(): void {}
//       ^^^^^ def
if (true) { check(); }
//          ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_function_call_in_for_loop() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function ti$0ck(): void {}
//       ^^^^ def
for (tick(); $i < 10; tick()) {}
//   ^^^^ ref
//                    ^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_function_call_inside_switch_case() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function ti$0ck(): void {}
//       ^^^^ def
switch ($x) {
    case 1: tick(); break;
    //      ^^^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_call_inside_switch_case() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Proc {
    public function pro$0cess(): void {}
    //              ^^^^^^^ def
}
switch ($x) {
    case 1: $obj->process(); break;
    //            ^^^^^^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_function_call_inside_switch_condition() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function cla$0ssify(): string { return ''; }
//       ^^^^^^^^ def
switch (classify()) { default: break; }
//      ^^^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_function_call_inside_throw() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function makeEx$0ception(): \Exception { return new \Exception(); }
//       ^^^^^^^^^^^^^ def
throw makeException();
//    ^^^^^^^^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_call_inside_throw() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Factory {
    public function cre$0ate(): \Exception { return new \Exception(); }
    //              ^^^^^^ def
}
throw $factory->create();
//              ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_call_inside_unset() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Obj {
    public function getP$0rop(): mixed { return null; }
    //              ^^^^^^^ def
}
unset($obj->getProp());
//          ^^^^^^^ ref
"#,
    )
    .await;
}

/// PHP callable arrays encode method references in string literals:
/// `[Service::class, 'handle']`, `[$service, 'handle']`, and similar framework
/// callback registrations. PHPStorm includes these in find-usages; omitting
/// them makes controller/listener/action methods look unused even when they are
/// wired through a dispatcher.
#[tokio::test]
#[ignore = "known gap: method references do not include PHP callable-array string method names"]
async fn references_method_includes_callable_array_string_entries() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Handler {
    public function han$0dle(): void {}
    //              ^^^^^^ def
}
$handler = new Handler();
$callable = [$handler, 'handle'];
//                       ^^^^^^ ref
$static = [Handler::class, 'handle'];
//                          ^^^^^^ ref
"#,
    )
    .await;
}
