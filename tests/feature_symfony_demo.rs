//! Workspace-scale smoke test for the vendored symfony/demo fixture.
//!
//! All tests are `#[ignore]` — run with `cargo test --release -- --ignored`
//! to verify the full symfony fixture end-to-end without running the
//! entire suite.

mod common;

use common::TestServer;

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn smoke_goto_definition_abstract_controller() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, line, character) = server.locate(path, "AbstractController", 1);
    server.open(path, &text).await;

    let resp = server.definition(path, line, character).await;
    assert!(resp["error"].is_null(), "definition error: {:?}", resp);

    let result = &resp["result"];
    assert!(!result.is_null(), "expected a definition location");
    let loc = if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    };
    let uri = loc["uri"]
        .as_str()
        .or_else(|| loc["targetUri"].as_str())
        .unwrap_or_default();
    assert!(
        uri.ends_with("/vendor/symfony/framework-bundle/Controller/AbstractController.php"),
        "definition should point to AbstractController, got: {uri}"
    );
}
