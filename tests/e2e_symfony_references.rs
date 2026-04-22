//! Find-references across the vendored symfony/demo tree. Asserts
//! structural lower bounds (e.g. at least N files mention the Post
//! entity) rather than exact counts, which drift whenever Symfony adds
//! or removes a call site in a new release.
//!
//! Run with `cargo test --release -- --ignored`.

mod common;

use common::TestServer;
use std::collections::HashSet;

#[tokio::test]
#[ignore = "slow: workspace-scale test, run with --ignored"]
async fn references_to_post_entity_span_multiple_files() {
    let mut server = TestServer::with_fixture("symfony-demo").await;
    server.wait_for_index_ready().await;

    // Anchor on the `class Post` declaration in src/Entity/Post.php so the
    // LSP resolves the symbol unambiguously.
    let path = "src/Entity/Post.php";
    let (text, line, character) = server.locate(path, "class Post", 0);
    // Land cursor on `Post`, past the `class ` keyword.
    let character = character + "class ".len() as u32;
    server.open(path, &text).await;

    let resp = server.references(path, line, character, false).await;
    assert!(resp["error"].is_null(), "references error: {:?}", resp);
    let locs = resp["result"].as_array().cloned().unwrap_or_default();

    let files: HashSet<String> = locs
        .iter()
        .filter_map(|l| l["uri"].as_str().map(|s| s.to_string()))
        .collect();

    // From `grep -rl "App\\Entity\\Post"` in src/ we see ~9 files reference
    // the entity. Require at least 4 to leave headroom for refactors.
    assert!(
        files.len() >= 4,
        "expected Post references across ≥4 files, got {} ({:?})",
        files.len(),
        files,
    );
    // Spot-check: the PostRepository file, which consumes the entity
    // heavily, must be represented.
    assert!(
        files
            .iter()
            .any(|u| u.ends_with("/src/Repository/PostRepository.php")),
        "PostRepository.php should be among references; files: {files:?}"
    );
}
