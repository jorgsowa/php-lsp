//! Call hierarchy + type hierarchy — all tests go through the LSP wire protocol.

mod common;

use common::TestServer;
use expect_test::expect;

// ── call hierarchy: prepare ────────────────────────────────────────────────────

#[tokio::test]
async fn prepare_function_returns_function_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
function gree$0t(): void {}
"#,
        )
        .await;
    expect!["greet (Function) @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_class_method_returns_method_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
class Mailer {
    public function sen$0d(): void {}
}
"#,
        )
        .await;
    expect!["send (Method) [Mailer] @ main.php:2"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_trait_method_returns_method_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
trait Timestampable {
    public function touc$0h(): void {}
}
"#,
        )
        .await;
    expect!["touch (Method) [Timestampable] @ main.php:2"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_enum_method_returns_method_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
enum Suit {
    case Hearts;
    public function lab$0el(): string { return 'x'; }
}
"#,
        )
        .await;
    expect!["label (Method) [Suit] @ main.php:3"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_unknown_symbol_returns_empty() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
$va$0r = 42;
"#,
        )
        .await;
    expect!["<empty>"].assert_eq(&out);
}

// ── call hierarchy: incoming ───────────────────────────────────────────────────

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
async fn incoming_calls_empty_when_never_called() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function unuse$0d(): void {}
"#,
        )
        .await;
    expect!["<no calls>"].assert_eq(&out);
}

#[tokio::test]
async fn incoming_calls_multiple_callers() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function tar$0get(): void {}
function a(): void { target(); }
function b(): void { target(); }
"#,
        )
        .await;
    expect!["a @ main.php:2\nb @ main.php:3"].assert_eq(&out);
}

#[tokio::test]
async fn incoming_calls_cross_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"//- /Service.php
<?php function proces$0s(): void {}
//- /Controller.php
<?php function handle(): void { process(); }
"#,
        )
        .await;
    expect!["handle @ Controller.php:0"].assert_eq(&out);
}

#[tokio::test]
async fn incoming_calls_from_file_scope() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function boota$0ble(): void {}
bootable();
"#,
        )
        .await;
    expect!["<file scope> @ main.php:2"].assert_eq(&out);
}

// ── call hierarchy: outgoing ───────────────────────────────────────────────────

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

#[tokio::test]
async fn outgoing_calls_empty_for_leaf_function() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function noo$0p(): void { $x = 1; }
"#,
        )
        .await;
    expect!["<no calls>"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_cross_file_callee() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"//- /main.php
<?php function orchest$0rate(): void { helper(); }
//- /helpers.php
<?php function helper(): void {}
"#,
        )
        .await;
    expect!["helper @ helpers.php:0"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_deduplicates_repeated_callee() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function helper(): void {}
function caller$0(): void { helper(); helper(); }
"#,
        )
        .await;
    expect!["helper @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_from_class_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function validate(): bool { return true; }
class Order {
    public function subm$0it(): void { validate(); }
}
"#,
        )
        .await;
    expect!["validate @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_from_enum_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function fmt(): string { return ''; }
enum Suit {
    public function lab$0el(): string { return fmt(); }
}
"#,
        )
        .await;
    expect!["fmt @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_includes_for_init_and_update() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function start(): int { return 0; }
function step(): void {}
function mai$0n(): void { for ($i = start(); $i < 10; step()) {} }
"#,
        )
        .await;
    expect!["start @ main.php:1\nstep @ main.php:2"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_includes_static_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
class Cache {
    public static function warm(): void {}
}
function bootstra$0p(): void { Cache::warm(); }
"#,
        )
        .await;
    expect!["warm @ main.php:2"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_inside_do_while() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function tick(): bool { return true; }
function pol$0l(): void { do {} while (tick()); }
"#,
        )
        .await;
    expect!["tick @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_inside_switch() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function action(): void {}
function dispa$0tch(int $x): void {
    switch ($x) {
        case 1: action(); break;
    }
}
"#,
        )
        .await;
    expect!["action @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_includes_args_of_new_expr() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function defaults(): array { return []; }
class Config {}
function boo$0t(): void { $c = new Config(defaults()); }
"#,
        )
        .await;
    expect!["defaults @ main.php:1"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_inside_cast() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function measure(): float { return 1.5; }
function conv$0ert(): int { return (int) measure(); }
"#,
        )
        .await;
    expect!["measure @ main.php:1"].assert_eq(&out);
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
