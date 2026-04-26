//! Type hierarchy against vendored symfony/demo.
//!
//! - supertypes of `BlogController` must include `AbstractController`
//! - subtypes of `AbstractController` must include at least one of the
//!   App controllers
//!
//! Run with `cargo test --release -- --ignored`.

mod common;

use common::TestServer;

fn item_names(items: &[serde_json::Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| i["name"].as_str().map(|s| s.to_string()))
        .collect()
}

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn supertypes_of_blog_controller_include_abstract_controller() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, line, character) = server.locate(path, "class BlogController", 0);
    let character = character + "class ".len() as u32;
    server.open(path, &text).await;

    let prep = server.prepare_type_hierarchy(path, line, character).await;
    assert!(
        prep["error"].is_null(),
        "prepareTypeHierarchy error: {:?}",
        prep
    );
    let items = prep["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !items.is_empty(),
        "prepareTypeHierarchy returned no items for BlogController"
    );

    let supers = server.supertypes(items[0].clone()).await;
    assert!(supers["error"].is_null(), "supertypes error: {:?}", supers);
    let arr = supers["result"].as_array().cloned().unwrap_or_default();
    let names = item_names(&arr);
    assert!(
        names.iter().any(|n| n == "AbstractController"),
        "supertypes of BlogController should include AbstractController; got: {names:?}"
    );
}

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn subtypes_of_abstract_controller_include_app_controller() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    // Open the vendor AbstractController so the LSP indexes it as the
    // hierarchy anchor.
    let path = "vendor/symfony/framework-bundle/Controller/AbstractController.php";
    let (text, line, character) = server.locate(path, "abstract class AbstractController", 0);
    let character = character + "abstract class ".len() as u32;
    server.open(path, &text).await;

    let prep = server.prepare_type_hierarchy(path, line, character).await;
    assert!(
        prep["error"].is_null(),
        "prepareTypeHierarchy error: {:?}",
        prep
    );
    let items = prep["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !items.is_empty(),
        "prepareTypeHierarchy returned no items for AbstractController"
    );

    let subs = server.subtypes(items[0].clone()).await;
    assert!(subs["error"].is_null(), "subtypes error: {:?}", subs);
    let arr = subs["result"].as_array().cloned().unwrap_or_default();
    let names = item_names(&arr);
    // The subtypes list in a vendored Symfony project is huge — require
    // at least one App controller as a sanity check.
    assert!(
        names
            .iter()
            .any(|n| n == "BlogController" || n == "UserController" || n == "SecurityController"),
        "subtypes of AbstractController should include an App controller; got {} names",
        names.len()
    );
}
