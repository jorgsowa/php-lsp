mod common;

use common::TestServer;

/// Verify that `completionItem/resolve` is wired up end-to-end: request a
/// completion list, pick an item, resolve it, and check the `detail` field is
/// populated. This tests the resolve round-trip protocol; scenario coverage
/// (which items appear, in what order) lives in feature_completion.rs.
#[tokio::test]
async fn completion_resolve_returns_item() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function resolveMe(): void {}
resolveM$0
"#,
        )
        .await;
    let c = opened.cursor();

    let comp = server.completion(&c.path, c.line, c.character).await;
    let items = match &comp["result"] {
        v if v.is_array() => v.as_array().unwrap().to_vec(),
        v if v["items"].is_array() => v["items"].as_array().unwrap().to_vec(),
        _ => vec![],
    };
    assert!(
        !items.is_empty(),
        "expected completions for 'resolveM' prefix: {:?}",
        comp["result"]
    );

    let resolve_me = items
        .iter()
        .find(|i| i["label"].as_str() == Some("resolveMe"))
        .cloned()
        .expect("resolveMe must appear in completions for its own prefix");

    let resp = server.completion_resolve(resolve_me).await;

    assert!(
        resp["error"].is_null(),
        "completionItem/resolve error: {resp:?}"
    );
    assert!(resp["result"].is_object(), "expected resolved item object");
    let detail = resp["result"]["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("resolveMe"),
        "resolved item must have detail populated with the function signature: {:?}",
        resp["result"]
    );
}
