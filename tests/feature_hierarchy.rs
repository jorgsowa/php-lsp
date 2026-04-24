//! Call hierarchy + type hierarchy coverage.

mod common;

use common::TestServer;
use expect_test::expect;

// ── call hierarchy ────────────────────────────────────────────────────────────

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
    expect!["caller @ main.php:2"].assert_eq(&out);
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
    expect!["leaf @ main.php:1"].assert_eq(&out);
}

// ── type hierarchy: prepare ───────────────────────────────────────────────────

#[tokio::test]
async fn prepare_class_returns_class_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_type_hierarchy(
            r#"<?php
class My$0Class {}
"#,
        )
        .await;
    expect!["MyClass (Class) @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_interface_returns_interface_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_type_hierarchy(
            r#"<?php
interface Conta$0inable {}
"#,
        )
        .await;
    expect!["Containable (Interface) @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_enum_returns_enum_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_type_hierarchy(
            r#"<?php
enum Suit$0 { case Hearts; }
"#,
        )
        .await;
    expect!["Suit (Enum) @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_unknown_symbol_returns_empty() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_type_hierarchy(
            r#"<?php
$x = new Un$0known();
"#,
        )
        .await;
    expect!["<empty>"].assert_eq(&out);
}

// ── type hierarchy: supertypes ────────────────────────────────────────────────

#[tokio::test]
async fn supertypes_class_extends_parent() {
    let mut s = TestServer::new().await;
    let out = s
        .check_supertypes(
            r#"<?php
class Animal {}
class D$0og extends Animal {}
"#,
        )
        .await;
    expect!["Animal (Class) @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn supertypes_implements_multiple_interfaces() {
    let mut s = TestServer::new().await;
    let out = s
        .check_supertypes(
            r#"//- /Circle.php
<?php class Circle$0 implements Drawable, Serializable {}
//- /Drawable.php
<?php interface Drawable {}
//- /Serializable.php
<?php interface Serializable {}
"#,
        )
        .await;
    expect!["Drawable (Interface) @ Drawable.php:0\nSerializable (Interface) @ Serializable.php:0"]
        .assert_eq(&out);
}

#[tokio::test]
async fn supertypes_root_class_returns_empty() {
    let mut s = TestServer::new().await;
    let out = s
        .check_supertypes(
            r#"<?php
class Root$0 {}
"#,
        )
        .await;
    expect!["<empty>"].assert_eq(&out);
}

#[tokio::test]
async fn supertypes_multi_level_returns_direct_parent_only() {
    let mut s = TestServer::new().await;
    let out = s
        .check_supertypes(
            r#"//- /A.php
<?php class A {}
//- /B.php
<?php class B extends A {}
//- /C.php
<?php class C$0 extends B {}
"#,
        )
        .await;
    expect!["B (Class) @ B.php:0"].assert_eq(&out);
}

// ── type hierarchy: subtypes ──────────────────────────────────────────────────

#[tokio::test]
async fn subtypes_interface_returns_implementing_classes() {
    let mut s = TestServer::new().await;
    let out = s
        .check_subtypes(
            r#"//- /Loggable.php
<?php interface Loggable$0 {}
//- /Service.php
<?php class Service implements Loggable {}
"#,
        )
        .await;
    expect!["Service (Class) @ Service.php:0"].assert_eq(&out);
}

#[tokio::test]
async fn subtypes_class_returns_extending_subclasses() {
    let mut s = TestServer::new().await;
    let out = s
        .check_subtypes(
            r#"//- /Base.php
<?php class Base$0 {}
//- /ChildA.php
<?php class ChildA extends Base {}
//- /ChildB.php
<?php class ChildB extends Base {}
"#,
        )
        .await;
    expect!["ChildA (Class) @ ChildA.php:0\nChildB (Class) @ ChildB.php:0"].assert_eq(&out);
}

#[tokio::test]
async fn subtypes_leaf_class_returns_empty() {
    let mut s = TestServer::new().await;
    let out = s
        .check_subtypes(
            r#"<?php
class Leaf$0 extends Base {}
"#,
        )
        .await;
    expect!["<empty>"].assert_eq(&out);
}

#[tokio::test]
async fn subtypes_abstract_class_returns_concrete_impl() {
    let mut s = TestServer::new().await;
    let out = s
        .check_subtypes(
            r#"//- /AbstractRepo.php
<?php abstract class AbstractRepo$0 {}
//- /UserRepo.php
<?php class UserRepo extends AbstractRepo {}
"#,
        )
        .await;
    expect!["UserRepo (Class) @ UserRepo.php:0"].assert_eq(&out);
}

#[tokio::test]
async fn subtypes_trait_returns_using_classes() {
    let mut s = TestServer::new().await;
    let out = s
        .check_subtypes(
            r#"//- /Timestamps.php
<?php trait Timestamps$0 {}
//- /Post.php
<?php class Post { use Timestamps; }
"#,
        )
        .await;
    expect!["Post (Class) @ Post.php:0"].assert_eq(&out);
}
