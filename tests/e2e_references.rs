mod common;

use common::TestServer;
use serde_json::Value;

fn lines_of(locs: &[Value]) -> Vec<u32> {
    locs.iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect()
}

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
async fn references_include_declaration_returns_both() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function a$0dd(int $a, int $b): int { return $a + $b; }
add(1, 2);
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.references(&c.path, c.line, c.character, true).await;

    assert!(resp["error"].is_null());
    let locs = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        locs.len() >= 2,
        "expected declaration + call site: {locs:?}"
    );
}

/// Regression for issue #125: cursor on a method *declaration* must return
/// method references, not free-function references with the same name.
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

/// Multi-file variant of #125: method decl in file A must not pull in
/// free-function usages of the same name from file B.
#[tokio::test]
async fn references_on_method_decl_excludes_cross_file_free_function() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"//- /a.php
<?php
class C {
    public function a$0dd() {}
}

//- /b.php
<?php
function add() {}
add();
$c->add();
"#,
        )
        .await;
    let c = opened.cursor();

    let a_uri = server.uri("a.php");
    let b_uri = server.uri("b.php");

    let resp = server.references(&c.path, c.line, c.character, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    let hits: Vec<(String, u32)> = resp["result"]
        .as_array()
        .expect("array")
        .iter()
        .map(|l| {
            (
                l["uri"].as_str().unwrap().to_string(),
                l["range"]["start"]["line"].as_u64().unwrap() as u32,
            )
        })
        .collect();

    assert!(
        hits.contains(&(a_uri.clone(), 2)),
        "method decl a.php:2 missing: {hits:?}"
    );
    assert!(
        hits.contains(&(b_uri.clone(), 3)),
        "method call b.php:3 missing: {hits:?}"
    );
    assert!(
        !hits.contains(&(b_uri.clone(), 1)),
        "free-function decl b.php:1 must be excluded: {hits:?}"
    );
    assert!(
        !hits.contains(&(b_uri.clone(), 2)),
        "free-function call b.php:2 must be excluded: {hits:?}"
    );
}

/// The codebase fast path (`find_references_codebase`) for a `final` class
/// method across files. Uses `with_root` because the fast path relies on the
/// workspace scan populating the index.
#[tokio::test]
async fn references_fast_path_final_class_cross_file_e2e() {
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

/// Regression: references on `__construct` of class `Foo` must return only
/// Foo's constructor and its call sites (`new Foo(...)`), NOT every other
/// class's `__construct` declaration. The symbol has a class-scoped identity;
/// name-only matching across classes is wrong.
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

    let (text, _, _) = server.locate("a.php", "<?php", 0);
    server.open("a.php", &text).await;

    let (_, line, col) = server.locate("a.php", "__construct", 0);
    let resp = server.references("a.php", line, col + 2, true).await;
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

    // Must NOT include Bar's unrelated __construct declaration.
    assert!(
        !hits.contains(&(b_uri.clone(), 2)),
        "Bar::__construct decl on b.php:2 must be excluded — got {hits:?}"
    );
    // Must NOT include `new Bar('x')` call.
    assert!(
        !hits.contains(&(c_uri.clone(), 2)),
        "`new Bar('x')` on c.php:2 must be excluded — got {hits:?}"
    );
    // Sanity: Foo's own constructor and `new Foo(1)` should be present.
    assert!(
        hits.iter().any(|(u, _)| u == &a_uri),
        "Foo::__construct decl missing — got {hits:?}"
    );
    assert!(
        hits.contains(&(c_uri.clone(), 1)),
        "`new Foo(1)` missing from c.php:1 — got {hits:?}"
    );
}

/// Regression for Bug 1: two constructors in the same file — `str_offset`
/// would always find the first `__construct` occurrence, so the declaration
/// span for the second constructor pointed at the first one. With the fix the
/// cursor position is used directly, so each constructor gets its own span.
#[tokio::test]
async fn references_on_second_constructor_has_correct_decl_span() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
class Alpha {
    public function __construct(int $x) {}
}
class Beta {
    public function __con$0struct(string $s) {}
}
new Alpha(1);
new Beta('x');
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.references(&c.path, c.line, c.character, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    let hits: Vec<u32> = resp["result"]
        .as_array()
        .expect("array")
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect();

    // Beta's constructor is on line 5; the decl span must point there, not at
    // Alpha's constructor on line 2.
    assert!(
        hits.contains(&5),
        "Beta::__construct decl (line 5) missing: {hits:?}"
    );
    assert!(
        !hits.contains(&2),
        "Alpha::__construct decl (line 2) must not appear: {hits:?}"
    );
    // `new Beta('x')` is on line 8.
    assert!(
        hits.contains(&8),
        "`new Beta(...)` (line 8) missing: {hits:?}"
    );
    // `new Alpha(1)` must not appear.
    assert!(
        !hits.contains(&7),
        "`new Alpha(...)` (line 7) must not appear: {hits:?}"
    );
}

/// Regression for Bug 2: braced-namespace class `__construct` — the function
/// previously only walked top-level statements and skipped
/// `NamespaceBody::Braced`, returning `None` for every constructor inside a
/// braced namespace block and falling through to name-only matching.
#[tokio::test]
async fn references_on_constructor_in_braced_namespace() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
namespace Shop {
    class Order {
        public function __con$0struct(int $id) {}
    }
}
namespace Shop {
    $o = new Order(1);
}
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.references(&c.path, c.line, c.character, true).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    let hits: Vec<u32> = resp["result"]
        .as_array()
        .expect("array")
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect();

    // The constructor declaration is on line 3.
    assert!(
        hits.contains(&3),
        "Order::__construct decl (line 3) missing: {hits:?}"
    );
    // `new Order(1)` is on line 8.
    assert!(
        hits.contains(&7),
        "`new Order(1)` (line 7) missing: {hits:?}"
    );
}

/// Regression for Bug 3: two classes with the same short name in different
/// namespaces — the constructor path previously called `find_references_codebase`
/// with the bare short name, so `new Foo(...)` sites from *both* namespaces
/// were returned when asking for refs on one class's constructor.
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

    let (text, _, _) = server.locate("a.php", "<?php", 0);
    server.open("a.php", &text).await;

    let (_, line, col) = server.locate("a.php", "__construct", 0);
    let resp = server.references("a.php", line, col + 2, true).await;
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

    // `new \Alpha\Widget(1)` is on c.php line 1.
    assert!(
        hits.contains(&(c_uri.clone(), 1)),
        "`new \\Alpha\\Widget(1)` missing: {hits:?}"
    );
    // `new \Beta\Widget('x')` must NOT appear.
    assert!(
        !hits.contains(&(c_uri.clone(), 2)),
        "`new \\Beta\\Widget('x')` must not appear: {hits:?}"
    );
    // Beta's constructor declaration must NOT appear.
    assert!(
        !hits.iter().any(|(u, _)| u == &b_uri),
        "Beta::Widget::__construct must not appear: {hits:?}"
    );
}

#[tokio::test]
async fn references_finds_all_usages_of_function() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function a$0dd(int $a, int $b): int { return $a + $b; }
add(1, 2);
add(3, 4);
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.references(&c.path, c.line, c.character, true).await;

    assert!(resp["error"].is_null(), "references error: {resp:?}");
    let locs = resp["result"].as_array().expect("array");
    assert_eq!(
        locs.len(),
        3,
        "expected 3 refs (1 decl + 2 calls): {locs:?}"
    );
    let lines = lines_of(locs);
    assert!(lines.contains(&1), "decl line 1 missing");
    assert!(lines.contains(&2), "call line 2 missing");
    assert!(lines.contains(&3), "call line 3 missing");
}
