//! Constructor-specific reference tests: scope isolation, namespaces, exclusions, FQN ranges.

use super::*;
use expect_test::expect;

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
async fn references_constructor_fqn_range_covers_full_name() {
    // Regression: constructor references via FQN (`new \App\Widget()`) produced a
    // range covering only `short_name.len()` characters from the `\` in `\App\Widget`,
    // i.e. `\App\W` instead of the full `\App\Widget`.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /Widget.php
<?php
namespace App;
class Widget {
    public function __con$0struct() {}
    //              ^^^^^^^^^^^ def
}

//- /main.php
<?php
$w = new \App\Widget();
//       ^^^^^^^^^^^ ref
"#,
    )
    .await;
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

    let out = render_locations(&resp, &server.uri(""));
    expect![[r#"
        main.php:4:9-4:16
        main.php:5:9-5:16"#]]
    .assert_eq(&out);
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
    expect![[r#"
        a.php:2:20-2:31
        c.php:1:11-1:14"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
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
    expect![[r#"
        a.php:3:20-3:31
        c.php:1:9-1:22"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Cursor on `parent::__construct()` in a child constructor must resolve to
/// the *parent* class's own instantiation sites. `parent::` is compile-time
/// resolved in PHP — it always names the literal `extends` class, never
/// subject to late static binding — so it must not resolve to `new Child(...)`,
/// which invokes a different (overriding) constructor.
#[tokio::test]
async fn references_constructor_from_parent_call_site_scoped_to_parent() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Base {
    public function __construct(int $id) {}
}
class Child extends Base {
    public function __construct(int $id, string $name) {
        parent::__con$0struct($id);
        //      ^^^^^^^^^^^ ref
    }
}
new Child(1, 'Alice');
new Base(2);
//  ^^^^ ref
"#,
    )
    .await;
}

/// Cursor on `parent::__construct()` resolves to the parent class's own
/// instantiation sites, even when the child class itself is never
/// instantiated directly.
#[tokio::test]
async fn references_constructor_call_site_resolves_to_parent_instantiation() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Alpha {
    public function __construct() {}
}
class Beta extends Alpha {
    public function __construct() {
        parent::__con$0struct();
        //      ^^^^^^^^^^^ ref
    }
}
new Alpha();
//  ^^^^^ ref
"#,
    )
    .await;
}

/// Cursor on `parent::__construct()` inside a namespaced child class must be
/// scoped to that namespace and must not return instantiation sites for a
/// same-short-name class in a different namespace (braced-namespace style).
#[tokio::test]
async fn references_constructor_call_site_namespaced_class_excludes_sibling_namespace() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /a.php
<?php
namespace Alpha;
class Widget extends Base {
    public function __construct(int $x) {
        parent::__con$0struct($x);
    }
}

//- /b.php
<?php
namespace Beta;
class Widget {
    public function __construct(string $s) {}
}

//- /c.php
<?php
$a = new \Alpha\Widget(1);
//       ^^^^^^^^^^^^^ ref
$b = new \Beta\Widget('x');
"#,
    )
    .await;
}

/// Same as above but the namespace is declared via the simple (no-brace) style.
#[tokio::test]
async fn references_constructor_call_site_simple_namespace_excludes_sibling_namespace() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /a.php
<?php
namespace Alpha;
class Box extends Base {
    public function __construct(int $n) {
        parent::__con$0struct($n);
    }
}

//- /b.php
<?php
namespace Beta;
class Box {
    public function __construct(string $s) {}
}

//- /c.php
<?php
$a = new \Alpha\Box(1);
//       ^^^^^^^^^^ ref
$b = new \Beta\Box('x');
"#,
    )
    .await;
}

/// `parent::__construct()` resolution (`resolve_parent_construct_class`) is
/// keyed off `WorkspaceIndexData::classes_by_name`, which buckets every
/// class by *short name only*. With several unrelated classes named `Base`
/// scattered across the workspace, the lookup must still land on the one
/// FQN actually named in the `extends` clause — not an arbitrary same-named
/// decoy from a different namespace.
#[tokio::test]
async fn references_constructor_call_site_disambiguates_among_many_same_short_name_classes() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /alpha/base.php
<?php
namespace Alpha;
class Base {
    public function __construct(int $id) {}
}

//- /alpha/child.php
<?php
namespace Alpha;
class Child extends Base {
    public function __construct(int $id) {
        parent::__con$0struct($id);
        //      ^^^^^^^^^^^ ref
    }
}

//- /beta/base.php
<?php
namespace Beta;
class Base {
    public function __construct(string $s) {}
}

//- /gamma/base.php
<?php
namespace Gamma;
class Base {
    public function __construct(array $a) {}
}

//- /delta/base.php
<?php
namespace Delta;
class Base {
    public function __construct(float $f) {}
}

//- /usage.php
<?php
new \Alpha\Base(1);
//  ^^^^^^^^^^^ ref
new \Beta\Base('x');
new \Gamma\Base([]);
new \Delta\Base(1.5);
"#,
    )
    .await;
}

/// Cursor on `__construct` in the constructor body of a class with a different
/// name must not bleed into sibling-class constructor references.
#[tokio::test]
async fn references_constructor_decl_does_not_include_sibling_class() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Lion {
    public function __con$0struct() {}
    //              ^^^^^^^^^^^ def
}
class Tiger {
    public function __construct() {}
}
new Lion();
//  ^^^^ ref
new Tiger();
"#,
    )
    .await;
}
