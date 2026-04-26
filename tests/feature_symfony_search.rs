//! Workspace + document symbol search across vendored symfony/demo.
//! Run with `cargo test --release -- --ignored`.

mod common;

use common::TestServer;

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn workspace_symbols_finds_controller_by_exact_name() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let resp = server.workspace_symbols("BlogController").await;
    assert!(
        resp["error"].is_null(),
        "workspace/symbol error: {:?}",
        resp
    );
    let items = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !items.is_empty(),
        "expected at least one symbol for query 'BlogController'"
    );
    let found_app_controller = items.iter().any(|it| {
        it["location"]["uri"]
            .as_str()
            .map(|u| u.contains("/src/Controller/") && u.ends_with("BlogController.php"))
            .unwrap_or(false)
    });
    assert!(
        found_app_controller,
        "no result pointed at src/Controller/BlogController.php; got: {:?}",
        items
    );
}

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn workspace_symbols_fuzzy_prefix() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let resp = server.workspace_symbols("Blog").await;
    assert!(resp["error"].is_null());
    let items = resp["result"].as_array().cloned().unwrap_or_default();
    let has_blog_controller = items.iter().any(|it| {
        it["name"]
            .as_str()
            .map(|n| n.contains("BlogController"))
            .unwrap_or(false)
    });
    assert!(
        has_blog_controller,
        "fuzzy query 'Blog' should surface BlogController; got {} items",
        items.len()
    );
}

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn document_symbols_lists_blog_controller_methods() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, _, _) = server.locate(path, "class BlogController", 0);
    server.open(path, &text).await;

    let resp = server.document_symbols(path).await;
    assert!(resp["error"].is_null(), "documentSymbol error: {:?}", resp);

    let result = &resp["result"];
    assert!(result.is_array(), "expected symbol array");
    let names = collect_names(result);

    for expected in ["BlogController", "index", "postShow", "commentNew"] {
        assert!(
            names.iter().any(|n| n == expected),
            "documentSymbol missing {expected}; got: {names:?}"
        );
    }
}

fn collect_names(v: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_names_into(v, &mut out);
    out
}

fn collect_names_into(v: &serde_json::Value, out: &mut Vec<String>) {
    if let Some(arr) = v.as_array() {
        for el in arr {
            if let Some(name) = el["name"].as_str() {
                out.push(name.to_string());
            }
            if let Some(children) = el.get("children") {
                collect_names_into(children, out);
            }
        }
    }
}
