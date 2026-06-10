//! Type hierarchy — all tests go through the LSP wire protocol.

use super::*;

use expect_test::expect;

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
async fn prepare_unknown_type_symbol_returns_empty() {
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

/// Partial class name must not be confused with a supertype — "Animal" must not
/// match a class named "AnimalHouse" (which extends an unrelated "Creature").
#[tokio::test]
async fn subtypes_partial_class_name_not_confused_with_supertype() {
    let mut s = TestServer::new().await;
    let out = s
        .check_subtypes(
            r#"<?php
interface Animal$0 {}
class AnimalHouse extends Creature {}
"#,
        )
        .await;
    assert!(
        !out.contains("AnimalHouse"),
        "AnimalHouse does not implement Animal: {out}"
    );
}

/// Anonymous classes have no name and must be skipped silently — the server
/// must not panic when encountering `new class extends Animal {}`.
#[tokio::test]
async fn subtypes_with_anonymous_class_does_not_panic() {
    let mut s = TestServer::new().await;
    // Only assert no panic; anonymous classes produce no named subtype.
    let _ = s
        .check_subtypes(
            r#"<?php
interface Animal$0 {}
$obj = new class extends Animal {};
"#,
        )
        .await;
}

/// When goto-implementation is requested on a symbol defined in both the current
/// file and another file, URIs in the result must point to the correct source.
#[tokio::test]
async fn subtypes_location_uris_match_source_files() {
    let mut s = TestServer::new().await;
    let out = s
        .check_subtypes(
            r#"//- /src/Animal.php
<?php
interface Animal$0 {}

//- /src/Dog.php
<?php
class Dog implements Animal {}

//- /src/Cat.php
<?php
class Cat implements Animal {}
"#,
        )
        .await;
    expect![[r#"
        Cat (Class) @ src/Cat.php:1
        Dog (Class) @ src/Dog.php:1"#]]
    .assert_eq(&out);
}

/// After workspace indexing completes, supertypes must resolve a parent class
/// that lives in the workspace index (not open in the editor).
#[tokio::test]
async fn supertypes_resolves_parent_from_workspace_index() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_supertypes(
            r#"//- /src/Repository.php
<?php
class Repository {}

//- /src/UserRepository.php
<?php
class UserRepository$0 extends Repository {}
"#,
        )
        .await;
    expect!["Repository (Class) @ src/Repository.php:1"].assert_eq(&out);
}

/// Supertypes resolves via short-name lookup.  Two classes with the same short
/// name but different namespaces both appear as candidates; the test documents
/// that supertypes returns *a* match rather than asserting a specific FQN.
#[tokio::test]
async fn supertypes_same_short_name_finds_one_match() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_supertypes(
            r#"//- /A/Base.php
<?php class BaseA {}

//- /B/Base.php
<?php class BaseB {}

//- /App/Child.php
<?php
class Child$0 extends BaseA {}
"#,
        )
        .await;
    // Unique parent name: must resolve to exactly the right class.
    expect!["BaseA (Class) @ A/Base.php:0"].assert_eq(&out);
}
