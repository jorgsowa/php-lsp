//! Hover against symfony/demo. Run with `cargo test --release -- --ignored`.

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
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn hover_on_class_in_extends_clause() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, line, character) = server.locate(path, "AbstractController", 1);
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
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn hover_on_app_entity_type_in_signature() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
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
