mod common;

use common::TestServer;

#[tokio::test]
async fn references_with_exclude_declaration() {
    let mut server = TestServer::new().await;
    server
        .open(
            "refs.php",
            "<?php\nfunction sub(int $a, int $b): int { return $a - $b; }\nsub(10, 3);\n",
        )
        .await;

    let resp = server.references("refs.php", 1, 9, false).await;

    assert!(resp["error"].is_null(), "references error: {:?}", resp);
    let result = &resp["result"];
    assert!(result.is_array(), "expected an array, got: {:?}", result);
    let locs = result.as_array().unwrap();
    assert_eq!(
        locs.len(),
        1,
        "expected exactly 1 call-site reference, got: {:?}",
        locs
    );
    assert_eq!(locs[0]["range"]["start"]["line"].as_u64().unwrap(), 2);
    assert_eq!(locs[0]["range"]["start"]["character"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn references_include_declaration_returns_both() {
    let mut server = TestServer::new().await;
    server
        .open(
            "refs_incl.php",
            "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
        )
        .await;

    let resp = server.references("refs_incl.php", 1, 9, true).await;

    assert!(resp["error"].is_null());
    let locs = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        locs.len() >= 2,
        "expected declaration + call site, got: {:?}",
        locs
    );
}

/// Regression test for issue #125: cursor on a method *declaration* must
/// return method references, not free-function references with the same name.
#[tokio::test]
async fn references_on_method_decl_returns_method_refs_not_function_refs() {
    let src =
        "<?php\nfunction add() {}\nclass C {\n    public function add() {}\n}\nadd();\n$c->add();";

    let mut server = TestServer::new().await;
    server.open("refs_test.php", src).await;

    let resp = server.references("refs_test.php", 3, 20, true).await;

    assert!(
        resp["error"].is_null(),
        "references should not error: {:?}",
        resp
    );
    let locs = resp["result"]
        .as_array()
        .expect("expected array of locations");
    let lines: Vec<u32> = locs
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect();

    assert!(
        lines.contains(&3),
        "method declaration (line 3) must be included, got: {:?}",
        lines
    );
    assert!(
        lines.contains(&6),
        "method call (line 6) must be included, got: {:?}",
        lines
    );
    assert!(
        !lines.contains(&1),
        "free-function declaration (line 1) must be excluded, got: {:?}",
        lines
    );
    assert!(
        !lines.contains(&5),
        "free-function call (line 5) must be excluded, got: {:?}",
        lines
    );

    let resp2 = server.references("refs_test.php", 3, 20, false).await;

    assert!(
        resp2["error"].is_null(),
        "references (no decl) should not error: {:?}",
        resp2
    );

    let lines2: Vec<u32> = resp2["result"]
        .as_array()
        .expect("expected array of locations")
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect();

    assert!(
        lines2.contains(&6),
        "method call (line 6) must be included when includeDeclaration=false, got: {:?}",
        lines2
    );
    assert!(
        !lines2.contains(&3),
        "method declaration (line 3) must be excluded when includeDeclaration=false, got: {:?}",
        lines2
    );
}

/// Multi-file variant of issue #125: method decl in file A must not pull in
/// free-function usages of the same name from file B.
#[tokio::test]
async fn references_on_method_decl_excludes_cross_file_free_function() {
    let src_a = "<?php\nclass C {\n    public function add() {}\n}";
    let src_b = "<?php\nfunction add() {}\nadd();\n$c->add();";

    let mut server = TestServer::new().await;
    server.open("a.php", src_a).await;
    server.open("b.php", src_b).await;

    let a_uri = server.uri("a.php");
    let b_uri = server.uri("b.php");

    let resp = server.references("a.php", 2, 20, true).await;

    assert!(
        resp["error"].is_null(),
        "references should not error: {:?}",
        resp
    );

    let locs = resp["result"]
        .as_array()
        .expect("expected array of locations");

    let hits: Vec<(String, u32)> = locs
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
        "method declaration (a.php line 2) must be included, got: {:?}",
        hits
    );
    assert!(
        hits.contains(&(b_uri.clone(), 3)),
        "method call (b.php line 3) must be included, got: {:?}",
        hits
    );
    assert!(
        !hits.contains(&(b_uri.clone(), 1)),
        "free-function declaration (b.php line 1) must be excluded, got: {:?}",
        hits
    );
    assert!(
        !hits.contains(&(b_uri.clone(), 2)),
        "free-function call (b.php line 2) must be excluded, got: {:?}",
        hits
    );
}

/// E2E: the codebase fast path (find_references_codebase) is exercised for a
/// `final` class method across multiple files.
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

    assert!(
        resp["error"].is_null(),
        "references should not error: {:?}",
        resp
    );

    let locs = resp["result"].as_array().expect("expected location array");
    let uris: Vec<&str> = locs.iter().map(|l| l["uri"].as_str().unwrap()).collect();

    assert!(
        uris.iter().any(|u| *u == caller_uri.as_str()),
        "caller.php (typed call) must appear in results, got: {:?}",
        uris
    );
    assert!(
        !uris.iter().any(|u| *u == ignored_uri.as_str()),
        "ignored.php (untyped call) must be excluded by the fast path, got: {:?}",
        uris
    );
}

#[tokio::test]
async fn references_finds_all_usages_of_function() {
    let mut server = TestServer::new().await;
    server
        .open(
            "refs_all.php",
            "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\nadd(3, 4);\n",
        )
        .await;

    let resp = server.references("refs_all.php", 1, 9, true).await;

    assert!(resp["error"].is_null(), "references error: {:?}", resp);
    let result = &resp["result"];
    assert!(result.is_array(), "expected array");
    let locs = result.as_array().unwrap();
    assert_eq!(
        locs.len(),
        3,
        "expected 3 references (1 declaration + 2 calls), got: {:?}",
        locs
    );
    let lines: Vec<u64> = locs
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert!(lines.contains(&1), "declaration on line 1 must be included");
    assert!(lines.contains(&2), "call on line 2 must be included");
    assert!(lines.contains(&3), "call on line 3 must be included");
}
