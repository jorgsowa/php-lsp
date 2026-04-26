//! Find-references across vendored symfony/demo. Run with
//! `cargo test --release -- --ignored`.

mod common;

use common::TestServer;
use std::collections::HashSet;

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn references_to_post_entity_span_multiple_files() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Entity/Post.php";
    let (text, line, character) = server.locate(path, "class Post", 0);
    let character = character + "class ".len() as u32;
    server.open(path, &text).await;

    let resp = server.references(path, line, character, false).await;
    assert!(resp["error"].is_null(), "references error: {:?}", resp);
    let locs = resp["result"].as_array().cloned().unwrap_or_default();

    let files: HashSet<String> = locs
        .iter()
        .filter_map(|l| l["uri"].as_str().map(|s| s.to_string()))
        .collect();

    assert!(
        files.len() >= 4,
        "expected Post references across ≥4 files, got {} ({:?})",
        files.len(),
        files,
    );
    assert!(
        files
            .iter()
            .any(|u| u.ends_with("/src/Repository/PostRepository.php")),
        "PostRepository.php should be among references; files: {files:?}"
    );
}
