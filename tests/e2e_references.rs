mod common;

use common::TestServer;
use serde_json::Value;

fn lines_of(locs: &[Value]) -> Vec<u32> {
    locs.iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect()
}

// ── Protocol-behaviour tests ─────────────────────────────────────────────────
// These tests exercise the `includeDeclaration` flag and other wire-level
// behaviours that the annotation-DSL tests in feature_references.rs cannot
// express. Scenario coverage (which symbols are returned for which cursors)
// lives in feature_references.rs.

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

/// Method decl on class C must not pull in free-function refs, and
/// `includeDeclaration=false` must exclude the method decl itself.
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
// These tests need the workspace index populated (with_root + wait_for_index_ready)
// because they exercise the codebase fast path that reads the workspace file index.

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

    // `submit` is on line 2, char 20
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
/// class's `__construct` declaration.
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

    // Open a.php so the server knows its content.
    server
        .open(
            "a.php",
            "<?php\nclass Foo {\n    public function __construct(int $x) {}\n}\n",
        )
        .await;

    // `__construct` is on line 2, char 20; col+2=22 places cursor inside it.
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

/// Regression: two classes with the same short name in different namespaces —
/// constructor path must use the FQN, not the bare short name.
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

    // Open a.php so the server knows its content.
    // `__construct` is on line 3, char 20; col+2=22 places cursor inside it.
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

/// Cross-file promoted property: accessing via `->prop` in another file must
/// be found when cursor is on the promoted param in the declaring file.
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

    // `$email` is on line 2, char 55; col+1=56 places cursor inside `$email`.
    // Position breakdown: "    public function __construct(public readonly string " = 55 chars.
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

/// Parallel warm must find exactly the right number of call sites across many
/// files — enough that the rayon thread pool actually distributes work.
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

    // `target` is on line 1, char 9
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

/// After the first references call populates salsa memos, a second call for
/// the same symbol must return the same result.
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
