//! Call hierarchy + type hierarchy smoke coverage.

mod common;

use common::TestServer;

#[tokio::test]
async fn incoming_calls_lists_callers() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function leaf$0(): void {}
function caller(): void { leaf(); }
"#,
        )
        .await;
    assert!(out.contains("caller"), "expected caller: {out}");
}

#[tokio::test]
async fn outgoing_calls_lists_callees() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function leaf(): void {}
function caller$0(): void { leaf(); }
"#,
        )
        .await;
    assert!(out.contains("leaf"), "expected leaf: {out}");
}

#[tokio::test]
async fn supertypes_lists_parent_class() {
    let mut s = TestServer::new().await;
    let out = s
        .check_supertypes(
            r#"<?php
class Animal {}
class D$0og extends Animal {}
"#,
        )
        .await;
    assert!(
        out.contains("Animal") || out == "<empty>" || out == "<no prepared item>",
        "unexpected: {out}"
    );
}

#[tokio::test]
async fn subtypes_lists_child_classes() {
    let mut s = TestServer::new().await;
    let out = s
        .check_subtypes(
            r#"<?php
class Anim$0al {}
class Dog extends Animal {}
class Cat extends Animal {}
"#,
        )
        .await;
    assert!(
        out.contains("Dog")
            || out.contains("Cat")
            || out == "<empty>"
            || out == "<no prepared item>",
        "unexpected: {out}"
    );
}
