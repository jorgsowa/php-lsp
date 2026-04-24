mod common;

use common::TestServer;

#[tokio::test]
async fn completion_after_initialize() {
    let mut server = TestServer::new().await;
    let opened = server.open_fixture("<?php\nclas$0").await;
    let c = opened.cursor();

    let resp = server.completion(&c.path, c.line, c.character).await;

    assert!(resp["error"].is_null(), "completion error: {resp:?}");
    let items = match &resp["result"] {
        v if v.is_array() => v.as_array().unwrap().clone(),
        v if v["items"].is_array() => v["items"].as_array().unwrap().clone(),
        other => panic!("completion must return a concrete list, got: {other:?}"),
    };
    assert!(
        !items.is_empty(),
        "top-level completion after `clas` must offer at least the `class` keyword"
    );
    assert!(
        items.iter().any(|i| i["label"].as_str() == Some("class")),
        "`class` keyword must be among completions: {items:?}"
    );
}

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
