//! Call hierarchy against vendored symfony/demo.
//!
//! Scenario: the controller's `index()` method is a known caller of
//! `PostRepository::findLatest()`. We prepare call hierarchy on
//! `findLatest`, then ask for incoming calls and assert the BlogController
//! caller is represented.
//!
//! Run with `cargo test --release -- --ignored`.

mod common;

use common::TestServer;

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn incoming_calls_to_post_repository_find_latest() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    // Open the definition site so the LSP knows where the method lives.
    let path = "src/Repository/PostRepository.php";
    let (text, line, character) = server.locate(path, "function findLatest", 0);
    // Land cursor on `findLatest`, past the `function ` keyword.
    let character = character + "function ".len() as u32;
    server.open(path, &text).await;

    // Open one caller too — call_hierarchy can only surface callers whose
    // source is in the index/open set.
    let caller_path = "src/Controller/BlogController.php";
    let (caller_text, _, _) = server.locate(caller_path, "class BlogController", 0);
    server.open(caller_path, &caller_text).await;

    let prep = server.prepare_call_hierarchy(path, line, character).await;
    assert!(
        prep["error"].is_null(),
        "prepareCallHierarchy error: {:?}",
        prep
    );
    let items = prep["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !items.is_empty(),
        "prepareCallHierarchy returned no items for findLatest"
    );

    let incoming = server.incoming_calls(items[0].clone()).await;
    assert!(
        incoming["error"].is_null(),
        "incomingCalls error: {:?}",
        incoming
    );
    let calls = incoming["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !calls.is_empty(),
        "expected at least one incoming caller of findLatest"
    );

    let from_blog_controller = calls.iter().any(|c| {
        c["from"]["uri"]
            .as_str()
            .map(|u| u.ends_with("/src/Controller/BlogController.php"))
            .unwrap_or(false)
    });
    assert!(
        from_blog_controller,
        "BlogController should be an incoming caller of findLatest; got: {calls:?}"
    );
}
