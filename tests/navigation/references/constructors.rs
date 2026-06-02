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
