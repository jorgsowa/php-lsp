//! Hover against symfony/demo — checks that signatures/types are surfaced
//! at realistic sites. Assertions look for *substrings* in the rendered
//! markdown; we deliberately avoid pinning the exact format so upstream
//! docblock tweaks don't break the suite.
//!
//! Run with `cargo test --release -- --ignored`.

mod common;

use common::TestServer;

fn hover_text(resp: &serde_json::Value) -> String {
    let c = &resp["result"]["contents"];
    if let Some(s) = c["value"].as_str() {
        return s.to_string();
    }
    if let Some(s) = c.as_str() {
        return s.to_string();
    }
    if let Some(arr) = c.as_array() {
        return arr
            .iter()
            .filter_map(|v| v["value"].as_str().or_else(|| v.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
    }
    String::new()
}

#[tokio::test]
#[ignore = "php-lsp gap: hover on class identifier in `extends` clause returns null (inline-fixture hover works on function names only)"]
async fn hover_on_class_in_extends_clause() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, line, character) = server.locate(path, "AbstractController", 1); // extends site
    server.open(path, &text).await;

    let resp = server.hover(path, line, character).await;
    assert!(resp["error"].is_null(), "hover error: {:?}", resp);
    assert!(
        !resp["result"].is_null(),
        "expected hover result on AbstractController"
    );
    let t = hover_text(&resp);
    assert!(
        t.contains("AbstractController"),
        "hover should mention AbstractController; got: {t}"
    );
}

#[tokio::test]
#[ignore = "php-lsp gap: hover on class name used as a parameter type returns null"]
async fn hover_on_app_entity_type_in_signature() {
    // Hover on `Post` in `Post $post` controller parameter.
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    // Skip occurrences in `use App\Entity\Post;` (0) and the `$post:post`
    // route parameter string (may or may not match our needle), landing on
    // the actual parameter declaration.
    let (text, line, character) = server.locate(path, "Post $post", 0);
    server.open(path, &text).await;

    let resp = server.hover(path, line, character).await;
    assert!(resp["error"].is_null(), "hover error: {:?}", resp);
    assert!(
        !resp["result"].is_null(),
        "expected a hover result on Post entity"
    );
    let t = hover_text(&resp);
    assert!(
        t.contains("Post") && (t.contains("class") || t.contains("App\\Entity")),
        "hover on Post should surface class info; got: {t}"
    );
}
