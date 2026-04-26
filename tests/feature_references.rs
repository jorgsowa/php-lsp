//! Comprehensive reference/find-usages coverage via the annotation DSL.
//!
//! Tests are written so the fixture itself specifies where references should
//! land — `// ^^^ def` for the declaration and `// ^^^ ref` for each use
//! site. `check_references_annotated` fails with a side-by-side diff if the
//! server returns anything missing or extra.

mod common;

use common::TestServer;

#[tokio::test]
async fn references_function_same_file() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function gr$0eet(): void {}
//       ^^^^^ def
greet();
//^^^^^ ref
greet();
//^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_same_file() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
    //              ^^^^^ def
}
$g = new Greeter();
$g->hello();
//  ^^^^^ ref
$g->hello();
//  ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_static_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Reg {
    public static function ge$0t(): void {}
    //                     ^^^ def
}
Reg::get();
//   ^^^ ref
Reg::get();
//   ^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_cross_file_via_use() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /src/Greeter.php
<?php
namespace App;
class Greeter {
    public function hel$0lo(): string { return 'hi'; }
    //              ^^^^^ def
}

//- /src/main.php
<?php
use App\Greeter;
$g = new Greeter();
$g->hello();
//  ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_no_usages_for_unused_function() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
function un$0used(): void {}
//       ^^^^^^ def
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_used_in_new() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Wi$0dget {}
//    ^^^^^^ def
$a = new Widget();
//       ^^^^^^ ref
$b = new Widget();
//       ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_distinguishes_like_named_methods() {
    // Two classes both define `process()`. Refs on Mailer::process must NOT
    // pick up Queue::process calls.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Mailer {
    public function pro$0cess(): void {}
    //              ^^^^^^^ def
}
class Queue {
    public function process(): void {}
}
$m = new Mailer();
$m->process();
//  ^^^^^^^ ref
$q = new Queue();
$q->process();
"#,
    )
    .await;
}

#[tokio::test]
async fn references_distinguishes_cross_namespace_functions() {
    // Two functions `greet` in different namespaces. Refs on `App\greet` must
    // NOT pick up the call to `Domain\greet`.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /src/app.php
<?php
namespace App;
function gr$0eet(): void {}
//       ^^^^^ def
greet();
//^^^^^ ref

//- /src/domain.php
<?php
namespace Domain;
function greet(): void {}
greet();
"#,
    )
    .await;
}

#[tokio::test]
async fn references_distinguishes_cross_namespace_classes() {
    // Two classes `User` in different namespaces. Refs on `App\User` must NOT
    // include the `new Domain\User()` site.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /src/app.php
<?php
namespace App;
class Us$0er {}
//    ^^^^ def
$a = new User();
//       ^^^^ ref

//- /src/domain.php
<?php
namespace Domain;
class User {}
$b = new User();
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_via_subclass_receiver_found() {
    // Method defined on a base class must also find calls on subclass receivers.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Base {
    public function wo$0rk(): void {}
    //              ^^^^ def
}
class Child extends Base {}
$c = new Child();
$c->work();
//  ^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_trait_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
trait Timestampable {
    public function touc$0hAt(): void {}
    //              ^^^^^^^ def
}
class Post {
    use Timestampable;
}
$p = new Post();
$p->touchAt();
//  ^^^^^^^ ref
$p->touchAt();
//  ^^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_interface_method_finds_call_sites() {
    // Cursor on the interface method declaration: must find both the
    // implementing class's method declaration and call sites.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
interface Renderable {
    public function ren$0der(): string;
    //              ^^^^^^ def
}
class Page implements Renderable {
    public function render(): string { return ''; }
    //              ^^^^^^ def
}
$page = new Page();
echo $page->render();
//         ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_enum_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
enum Status {
    case Active;
    public function lab$0el(): string { return 'active'; }
    //              ^^^^^ def
}
echo Status::Active->label();
//                   ^^^^^ ref
echo Status::Active->label();
//                   ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_nullsafe_method_call() {
    // `$obj?->method()` must be found as a reference alongside `$obj->method()`.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Mailer {
    public function se$0nd(): void {}
    //              ^^^^ def
}
$m = new Mailer();
$m->send();
//  ^^^^ ref
$m?->send();
//   ^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_includes_type_hints_and_extends() {
    // When cursor is on a class name (not __construct), refs include structural
    // usages: type hints, `extends`, and `instanceof`. No `new Ev$0ent()` is
    // present so the codebase fast path (which only tracks instantiation sites)
    // falls back to the AST walker that catches all class references.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Ev$0ent {}
//    ^^^^^ def
class UserEvent extends Event {}
//                      ^^^^^ ref
function dispatch(Event $e): void {}
//                ^^^^^ ref
$e = null;
if ($e instanceof Event) {}
//                ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_type_hint_with_new_call() {
    // When a class appears both as a type hint AND in a new expression, find-references
    // must include ALL sites — not just the new call. This is the regression case where
    // the salsa fast path returned only `new Widget()` and silently dropped type hints.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Wi$0dget {}
//    ^^^^^^ def
function foo(Widget $w): Widget {}
//           ^^^^^^ ref
//                       ^^^^^^ ref
$x = new Widget();
//       ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_constructor_decl_span_scoped_to_owning_class() {
    // Bug 1: two constructors in the same file — the decl span for Beta's
    // __construct must point at Beta (line 5), not Alpha (line 2).
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Alpha {
    public function __construct(int $x) {}
}
class Beta {
    public function __con$0struct(string $s) {}
    //              ^^^^^^^^^^^ def
}
new Alpha(1);
new Beta('x');
//  ^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_constructor_in_braced_namespace() {
    // Bug 2: braced-namespace constructor must be found by references.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
namespace Shop {
    class Order {
        public function __con$0struct(int $id) {}
        //              ^^^^^^^^^^^ def
    }
}
namespace Shop {
    $o = new Order(1);
    //       ^^^^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_constructor_excludes_type_hints_and_instanceof() {
    // __construct references must only include `new` call sites — not type hints,
    // `instanceof`, or `::class`. The annotation DSL implicitly asserts exclusions:
    // any location the server returns that isn't annotated causes a diff failure.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Order {
    public function __con$0struct(int $id) {}
    //              ^^^^^^^^^^^ def
}
$o = new Order(1);
//       ^^^^^ ref
function ship(Order $o): void {}
if ($o instanceof Order) {}
Order::class;
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_excludes_cross_file_free_function() {
    // Method refs on C::add must not include the free-function `add()`.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /a.php
<?php
class C {
    public function a$0dd() {}
    //              ^^^ def
}

//- /b.php
<?php
function add() {}
add();
$c->add();
//  ^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_promoted_property_this_access() {
    // `$this->prop` inside a method must be returned alongside external `->prop`
    // accesses when cursor is on a promoted constructor property.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Person {
    public function __construct(public readonly string $na$0me) {}
    public function greet(): string {
        return $this->name;
        //            ^^^^ ref
    }
}
$p = new Person('Alice');
echo $p->name;
//       ^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_promoted_property_finds_nullsafe_access() {
    // `$obj?->prop` must be returned alongside `$obj->prop` when searching
    // refs on a promoted constructor property. The promoted param declaration
    // itself is not included because the property walker finds access sites,
    // not constructor parameter declarations.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Config {
    public function __construct(public readonly string $ke$0y) {}
}
$c = new Config('x');
echo $c->key;
//       ^^^ ref
echo $c?->key;
//         ^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_on_unopened_uri_returns_empty() {
    let mut s = TestServer::new().await;
    let resp = s.references("ghost.php", 0, 0, false).await;
    assert!(resp["error"].is_null(), "references errored: {resp:?}");
    let result = &resp["result"];
    let is_empty = result.is_null() || result.as_array().map(|a| a.is_empty()).unwrap_or(false);
    assert!(
        is_empty,
        "references on unopened file should be empty, got: {result:?}"
    );
}
