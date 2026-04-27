//! Comprehensive reference/find-usages coverage via the annotation DSL.
//!
//! Tests are written so the fixture itself specifies where references should
//! land — `// ^^^ def` for the declaration and `// ^^^ ref` for each use
//! site. `check_references_annotated` fails with a side-by-side diff if the
//! server returns anything missing or extra.

mod common;

use common::TestServer;
use serde_json::Value;

fn lines_of(locs: &[Value]) -> Vec<u32> {
    locs.iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect()
}

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
    // accesses and the constructor param declaration when cursor is on a promoted
    // constructor property.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Person {
    public function __construct(public readonly string $na$0me) {}
    //                                                  ^^^^ ref
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
    // `$obj?->prop` must be returned alongside `$obj->prop` and the constructor
    // param declaration when searching refs on a promoted constructor property.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Config {
    public function __construct(public readonly string $ke$0y) {}
    //                                                  ^^^ ref
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

/// Searching references from a property *access* site (`$this->prop`) must
/// behave the same as searching from the constructor param declaration —
/// finding all property accesses, not method calls.
#[tokio::test]
async fn references_promoted_property_from_access_site() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Cart {
    public function __construct(private object $item) {}
    //                                          ^^^^ ref
    public function total(): void { $this->it$0em; }
    //                                      ^^^^ ref
    public function describe(): void { $this->item; }
    //                                        ^^^^ ref
}
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

/// Find-references on `class User` must surface `use App\Model\User` imports in
/// every dependent file. This is the safety-critical path rename depends on.
#[tokio::test]
async fn references_include_use_imports_across_files() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;
    let (text, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.open("src/Model/User.php", &text).await;

    let (_, line, ch) = server.locate("src/Model/User.php", "class User", 0);
    // Cursor on the `U` of `User` (after "class ").
    let resp = server
        .references("src/Model/User.php", line, ch + 6, false)
        .await;

    let refs = resp["result"].as_array().expect("references array");
    assert!(
        refs.len() >= 2,
        "expected at least 2 cross-file references, got {}",
        refs.len()
    );
    let ref_uris: Vec<&str> = refs.iter().filter_map(|r| r["uri"].as_str()).collect();
    assert!(
        ref_uris
            .iter()
            .any(|u| u.ends_with("src/Service/Registry.php")),
        "expected a reference in Registry.php, got: {ref_uris:?}"
    );
    assert!(
        ref_uris
            .iter()
            .any(|u| u.ends_with("src/Service/Greeter.php")),
        "expected a reference in Greeter.php, got: {ref_uris:?}"
    );
}

// ── Protocol-behaviour tests ─────────────────────────────────────────────────
// These tests exercise the `includeDeclaration` flag and other wire-level
// behaviours that the annotation-DSL tests above cannot express.

#[tokio::test]
async fn references_with_exclude_declaration() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function s$0ub(int $a, int $b): int { return $a - $b; }
sub(10, 3);
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.references(&c.path, c.line, c.character, false).await;

    assert!(resp["error"].is_null(), "references error: {resp:?}");
    let locs = resp["result"].as_array().expect("expected array").clone();
    assert_eq!(locs.len(), 1, "expected one call-site reference: {locs:?}");
    assert_eq!(locs[0]["range"]["start"]["line"].as_u64().unwrap(), 2);
    assert_eq!(locs[0]["range"]["start"]["character"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn references_on_constructor_with_include_declaration_false() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
class Invoice {
    public function __con$0struct(int $id) {}
}
$a = new Invoice(1);
$b = new Invoice(2);
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.references(&c.path, c.line, c.character, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    let hits: Vec<u32> = resp["result"]
        .as_array()
        .expect("expected array")
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect();

    assert!(
        hits.contains(&4),
        "`new Invoice(1)` (line 4) missing: {hits:?}"
    );
    assert!(
        hits.contains(&5),
        "`new Invoice(2)` (line 5) missing: {hits:?}"
    );
    assert!(
        !hits.contains(&2),
        "__construct decl (line 2) must be excluded when includeDeclaration=false: {hits:?}"
    );
    assert_eq!(hits.len(), 2, "expected exactly 2 call sites: {hits:?}");
}

#[tokio::test]
async fn references_on_method_decl_returns_method_refs_not_function_refs() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function add() {}
class C {
    public function a$0dd() {}
}
add();
$c->add();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.references(&c.path, c.line, c.character, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    let lines = lines_of(resp["result"].as_array().expect("array"));

    assert!(lines.contains(&3), "method decl line 3 missing: {lines:?}");
    assert!(lines.contains(&6), "method call line 6 missing: {lines:?}");
    assert!(
        !lines.contains(&1),
        "free-function decl line 1 must be excluded: {lines:?}"
    );
    assert!(
        !lines.contains(&5),
        "free-function call line 5 must be excluded: {lines:?}"
    );

    let resp2 = server.references(&c.path, c.line, c.character, false).await;
    assert!(resp2["error"].is_null(), "references error: {resp2:?}");
    let lines2 = lines_of(resp2["result"].as_array().expect("array"));
    assert!(
        lines2.contains(&6),
        "method call line 6 missing: {lines2:?}"
    );
    assert!(
        !lines2.contains(&3),
        "method decl must be excluded when includeDeclaration=false: {lines2:?}"
    );
}

// ── Workspace-scan / fast-path tests ─────────────────────────────────────────

#[tokio::test]
async fn references_fast_path_final_class_cross_file() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("class.php"),
        "<?php\nfinal class Order {\n    public function submit(): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("caller.php"),
        "<?php\n$order = new Order();\n$order->submit();\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("ignored.php"),
        "<?php\n$unknown->submit();\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    let caller_uri = server.uri("caller.php");
    let ignored_uri = server.uri("ignored.php");

    server
        .open(
            "class.php",
            "<?php\nfinal class Order {\n    public function submit(): void {}\n}\n",
        )
        .await;

    let resp = server.references("class.php", 2, 20, false).await;

    assert!(resp["error"].is_null(), "references error: {resp:?}");
    let uris: Vec<&str> = resp["result"]
        .as_array()
        .expect("array")
        .iter()
        .map(|l| l["uri"].as_str().unwrap())
        .collect();

    assert!(
        uris.iter().any(|u| *u == caller_uri.as_str()),
        "caller.php missing: {uris:?}"
    );
    assert!(
        !uris.iter().any(|u| *u == ignored_uri.as_str()),
        "ignored.php (untyped) must be excluded by fast path: {uris:?}"
    );
}

#[tokio::test]
async fn references_on_constructor_are_scoped_to_owning_class() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.php"),
        "<?php\nclass Foo {\n    public function __construct(int $x) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.php"),
        "<?php\nclass Bar {\n    public function __construct(string $s) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("c.php"),
        "<?php\n$foo = new Foo(1);\n$bar = new Bar('x');\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    server
        .open(
            "a.php",
            "<?php\nclass Foo {\n    public function __construct(int $x) {}\n}\n",
        )
        .await;

    let resp = server.references("a.php", 2, 22, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    let a_uri = server.uri("a.php");
    let b_uri = server.uri("b.php");
    let c_uri = server.uri("c.php");

    let hits: Vec<(String, u32)> = resp["result"]
        .as_array()
        .unwrap_or_else(|| panic!("expected array of references, got: {resp:?}"))
        .iter()
        .map(|l| {
            (
                l["uri"].as_str().unwrap().to_string(),
                l["range"]["start"]["line"].as_u64().unwrap() as u32,
            )
        })
        .collect();

    assert!(
        !hits.contains(&(b_uri.clone(), 2)),
        "Bar::__construct decl on b.php:2 must be excluded — got {hits:?}"
    );
    assert!(
        !hits.contains(&(c_uri.clone(), 2)),
        "`new Bar('x')` on c.php:2 must be excluded — got {hits:?}"
    );
    assert!(
        hits.iter().any(|(u, _)| u == &a_uri),
        "Foo::__construct decl missing — got {hits:?}"
    );
    assert!(
        hits.contains(&(c_uri.clone(), 1)),
        "`new Foo(1)` missing from c.php:1 — got {hits:?}"
    );
}

#[tokio::test]
async fn references_on_constructor_scoped_by_namespace_fqn() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a.php"),
        "<?php\nnamespace Alpha;\nclass Widget {\n    public function __construct(int $x) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("b.php"),
        "<?php\nnamespace Beta;\nclass Widget {\n    public function __construct(string $s) {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("c.php"),
        "<?php\n$a = new \\Alpha\\Widget(1);\n$b = new \\Beta\\Widget('x');\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    server
        .open(
            "a.php",
            "<?php\nnamespace Alpha;\nclass Widget {\n    public function __construct(int $x) {}\n}\n",
        )
        .await;

    let resp = server.references("a.php", 3, 22, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    let c_uri = server.uri("c.php");
    let b_uri = server.uri("b.php");

    let hits: Vec<(String, u32)> = resp["result"]
        .as_array()
        .unwrap_or_else(|| panic!("expected array, got: {resp:?}"))
        .iter()
        .map(|l| {
            (
                l["uri"].as_str().unwrap().to_string(),
                l["range"]["start"]["line"].as_u64().unwrap() as u32,
            )
        })
        .collect();

    assert!(
        hits.contains(&(c_uri.clone(), 1)),
        "`new \\Alpha\\Widget(1)` missing: {hits:?}"
    );
    assert!(
        !hits.contains(&(c_uri.clone(), 2)),
        "`new \\Beta\\Widget('x')` must not appear: {hits:?}"
    );
    assert!(
        !hits.iter().any(|(u, _)| u == &b_uri),
        "Beta::Widget::__construct must not appear: {hits:?}"
    );
}

#[tokio::test]
async fn references_on_promoted_property_cross_file() {
    let dir = tempfile::tempdir().unwrap();
    let entity_src = "<?php\nclass User {\n    public function __construct(public readonly string $email) {}\n}\n";
    std::fs::write(dir.path().join("entity.php"), entity_src).unwrap();
    std::fs::write(
        dir.path().join("service.php"),
        "<?php\nfunction notify(User $u): void {\n    echo $u->email;\n    echo $u?->email;\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    server.open("entity.php", entity_src).await;

    let resp = server.references("entity.php", 2, 56, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    let service_uri = server.uri("service.php");
    let hits: Vec<(String, u32)> = resp["result"]
        .as_array()
        .unwrap_or_else(|| panic!("expected array: {resp:?}"))
        .iter()
        .map(|l| {
            (
                l["uri"].as_str().unwrap().to_string(),
                l["range"]["start"]["line"].as_u64().unwrap() as u32,
            )
        })
        .collect();

    assert!(
        hits.contains(&(service_uri.clone(), 2)),
        "`$u->email` (service.php:2) missing: {hits:?}"
    );
    assert!(
        hits.contains(&(service_uri.clone(), 3)),
        "`$u?->email` (service.php:3) missing: {hits:?}"
    );
}

// ── Parallelism / consistency tests ──────────────────────────────────────────

#[tokio::test]
async fn parallel_warm_finds_all_references_across_many_files() {
    let dir = tempfile::tempdir().unwrap();
    let caller_count = 15usize;
    std::fs::write(
        dir.path().join("def.php"),
        "<?php\nfunction target(): void {}",
    )
    .unwrap();
    for i in 0..caller_count {
        std::fs::write(
            dir.path().join(format!("caller_{i}.php")),
            "<?php\ntarget();",
        )
        .unwrap();
    }
    for i in 0..5usize {
        std::fs::write(
            dir.path().join(format!("other_{i}.php")),
            format!("<?php\nfunction other_{i}() {{}}"),
        )
        .unwrap();
    }

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server
        .open("def.php", "<?php\nfunction target(): void {}")
        .await;

    let resp = server.references("def.php", 1, 9, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    let locs = resp["result"].as_array().expect("expected array");
    assert_eq!(
        locs.len(),
        caller_count,
        "expected {caller_count} references, got {}: {locs:?}",
        locs.len()
    );
}

#[tokio::test]
async fn parallel_warm_gives_consistent_results_on_repeated_references_calls() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"//- /a.php
<?php
function fo$0o(): void {}

//- /b.php
<?php
foo();

//- /c.php
<?php
foo(); foo();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp1 = server.references(&c.path, c.line, c.character, false).await;
    let resp2 = server.references(&c.path, c.line, c.character, false).await;

    let locs1 = resp1["result"].as_array().expect("array");
    let locs2 = resp2["result"].as_array().expect("array");
    assert_eq!(
        locs1.len(),
        3,
        "expected 3 references (1 from b.php, 2 from c.php): {locs1:?}"
    );
    assert_eq!(
        locs1.len(),
        locs2.len(),
        "repeated references calls returned different counts"
    );
}
