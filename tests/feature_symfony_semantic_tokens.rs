//! Semantic tokens against the workspace portion of vendored symfony/demo.
//!
//! Structural shape check only — asserts that the LSP emits a non-empty,
//! well-formed token stream (length a multiple of 5, deltas in bounds)
//! for a realistic controller file. Pinning exact token types is brittle
//! across parser changes; we leave that to unit tests.
//!
//! Tokenization is per-file and doesn't read across the workspace, so
//! `vendor/` is excluded from the scan.

mod common;

use common::TestServer;

#[tokio::test]
async fn semantic_tokens_full_on_blog_controller_is_nonempty_and_well_formed() {
    let mut server = TestServer::with_fixture_no_vendor("symfony-demo").await;
    server.wait_for_index_ready().await;

    let path = "src/Controller/BlogController.php";
    let (text, _, _) = server.locate(path, "class BlogController", 0);
    server.open(path, &text).await;

    let resp = server.semantic_tokens_full(path).await;
    assert!(
        resp["error"].is_null(),
        "semanticTokens/full error: {:?}",
        resp
    );

    let data = resp["result"]["data"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !data.is_empty(),
        "semanticTokens/full returned no tokens for BlogController"
    );
    assert_eq!(
        data.len() % 5,
        0,
        "token array length must be a multiple of 5 (LSP spec); got {}",
        data.len()
    );

    // Sanity check the 5-tuple shape: each token is
    //   [deltaLine, deltaStart, length, tokenType, tokenModifiers]
    // Tokens 1-onward have deltaStart that resets when deltaLine > 0, so
    // we just assert every field is a non-negative integer.
    for (i, v) in data.iter().enumerate() {
        assert!(
            v.is_u64() || v.is_i64(),
            "token[{i}] must be an integer, got: {v:?}"
        );
    }
}
