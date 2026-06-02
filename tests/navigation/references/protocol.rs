//! Protocol-behaviour tests: wire-level behaviors like includeDeclaration flag, unopened URIs.

use super::*;

use expect_test::expect;

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
    let out = render_locations(&resp, &server.uri(""));
    expect!["main.php:2:0-2:3"].assert_eq(&out);
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
    let out = render_locations(&resp, &server.uri(""));
    expect![[r#"
        main.php:3:20-3:23
        main.php:6:4-6:7"#]]
    .assert_eq(&out);

    let resp2 = server.references(&c.path, c.line, c.character, false).await;
    assert!(resp2["error"].is_null(), "references error: {resp2:?}");
    let out2 = render_locations(&resp2, &server.uri(""));
    expect!["main.php:6:4-6:7"].assert_eq(&out2);
}

#[tokio::test]
async fn references_after_did_change_reflects_added_call() {
    let mut server = TestServer::new().await;

    server
        .open(
            "main.php",
            "<?php\nfunction compute(): int { return 0; }\ncompute();\n",
        )
        .await;

    let resp1 = server.references("main.php", 1, 10, false).await;
    expect!["main.php:2:0-2:7"].assert_eq(&render_locations(&resp1, &server.uri("")));

    server
        .change(
            "main.php",
            2,
            "<?php\nfunction compute(): int { return 0; }\ncompute();\ncompute();\n",
        )
        .await;

    let resp2 = server.references("main.php", 1, 10, false).await;
    expect![[r#"
        main.php:2:0-2:7
        main.php:3:0-3:7"#]]
    .assert_eq(&render_locations(&resp2, &server.uri("")));
}
