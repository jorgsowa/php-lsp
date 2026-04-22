//! Workspace-scale E2E: goto-definition across the vendored symfony/demo
//! project. Proves the harness works against a real-world PHP codebase
//! (~6500 PHP files, full PSR-4 autoload, Symfony attributes, vendor tree).
//!
//! The fixture lives at `tests/fixtures/symfony-demo/` — see the README
//! there for provenance.

mod common;

use common::TestServer;

/// `BlogController extends AbstractController` → the declaration lives in
/// `vendor/symfony/framework-bundle/Controller/AbstractController.php`.
/// This exercises the PSR-4 / cross-file path: the token is resolved via
/// the `use` statement, then the autoloader maps the FQN to the file.
///
/// `#[ignore]`: the workspace scan of ~5,200 PHP files takes ~30 s even in
/// release mode, so this is opt-in. Run with:
///     cargo test --test e2e_symfony_demo --release -- --ignored
#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn goto_definition_abstract_controller() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    // BlogController.php line 40 (0-indexed 39):
    //   `final class BlogController extends AbstractController`
    // Column 40 lands inside `AbstractController`.
    let path = "src/Controller/BlogController.php";
    let text = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/symfony-demo")
            .join(path),
    )
    .unwrap();
    server.open(path, &text).await;

    let resp = server.definition(path, 39, 40).await;
    assert!(resp["error"].is_null(), "definition error: {:?}", resp);

    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected a definition location for AbstractController"
    );
    let loc = if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    };
    let uri = loc["uri"].as_str().unwrap_or_default();
    assert!(
        uri.ends_with("/vendor/symfony/framework-bundle/Controller/AbstractController.php"),
        "definition should point to AbstractController in vendor/, got: {uri}"
    );
}
