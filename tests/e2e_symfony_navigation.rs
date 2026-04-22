//! Goto-definition across the vendored symfony/demo project.
//!
//! Exercises resolution scenarios that inline fixtures can't hit:
//! - vendor-tree PSR-4 resolution
//! - Symfony attribute classes (`#[Route]`)
//! - method navigation inherited from an abstract base class in vendor/
//! - parameter types using `use`-imported FQNs
//!
//! All `#[ignore]`: run with `cargo test --release -- --ignored`.

mod common;

use common::TestServer;
use serde_json::Value;

/// Collapse a Location | Location[] | LocationLink[] response into one Location.
fn first_loc(result: &Value) -> Value {
    if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    }
}

fn uri_of(loc: &Value) -> &str {
    loc["uri"]
        .as_str()
        .or_else(|| loc["targetUri"].as_str())
        .unwrap_or_default()
}

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn goto_definition_parameter_type_in_vendor() {
    // `Request` in `index(Request $request, ...)` → vendor HttpFoundation.
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, line, character) = server.locate(path, "Request $request", 0);
    server.open(path, &text).await;

    let resp = server.definition(path, line, character).await;
    assert!(resp["error"].is_null(), "definition error: {:?}", resp);
    let loc = first_loc(&resp["result"]);
    assert!(!loc.is_null(), "expected Request class location");
    assert!(
        uri_of(&loc).ends_with("/vendor/symfony/http-foundation/Request.php"),
        "expected HttpFoundation/Request.php, got: {}",
        uri_of(&loc)
    );
}

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn goto_definition_app_class_from_use_import() {
    // `PostRepository $posts` in the controller signature → src/Repository/PostRepository.php.
    // Exercises project-local PSR-4, not vendor.
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, line, character) = server.locate(path, "PostRepository $posts", 0);
    server.open(path, &text).await;

    let resp = server.definition(path, line, character).await;
    assert!(resp["error"].is_null(), "definition error: {:?}", resp);
    let loc = first_loc(&resp["result"]);
    assert!(!loc.is_null(), "expected PostRepository location");
    assert!(
        uri_of(&loc).ends_with("/src/Repository/PostRepository.php"),
        "expected src/Repository/PostRepository.php, got: {}",
        uri_of(&loc)
    );
}

#[tokio::test]
#[ignore = "php-lsp gap: method goto-def on `$this->render` ignores receiver type; jumps to an unrelated render() (observed: BlockQuoteRenderer). Receiver-aware method dispatch is needed"]
async fn goto_definition_inherited_method_this_render() {
    // `$this->render(...)` in a controller method → render() on the vendor
    // AbstractController base class. This is method navigation across a
    // base-class hop into the vendor tree.
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, line, character) = server.locate(path, "$this->render(", 0);
    // The cursor needs to land on `render`, not on `$this->`.
    let character = character + "$this->".len() as u32 + 1; // +1 = inside "render"
    server.open(path, &text).await;

    let resp = server.definition(path, line, character).await;
    assert!(resp["error"].is_null(), "definition error: {:?}", resp);
    let loc = first_loc(&resp["result"]);
    assert!(
        !loc.is_null(),
        "expected a location for $this->render (inherited method)"
    );
    let uri = uri_of(&loc);
    assert!(
        uri.ends_with("/vendor/symfony/framework-bundle/Controller/AbstractController.php"),
        "expected AbstractController.php (defines render()), got: {uri}"
    );
}

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn goto_definition_attribute_class_route() {
    // `#[Route('/blog')]` on the class → Symfony\Component\Routing\Attribute\Route.
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, line, character) = server.locate(path, "#[Route(", 0);
    // Land the cursor inside `Route`, not on `#[`.
    let character = character + 2; // skip `#[`
    server.open(path, &text).await;

    let resp = server.definition(path, line, character).await;
    assert!(resp["error"].is_null(), "definition error: {:?}", resp);
    let loc = first_loc(&resp["result"]);
    assert!(!loc.is_null(), "expected Route attribute class location");
    let uri = uri_of(&loc);
    assert!(
        uri.ends_with("/vendor/symfony/routing/Attribute/Route.php"),
        "expected routing/Attribute/Route.php, got: {uri}"
    );
}
