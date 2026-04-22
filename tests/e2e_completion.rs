mod common;

use common::TestServer;

#[tokio::test]
async fn completion_after_initialize() {
    let mut server = TestServer::new().await;
    server.open("comp.php", "<?php\n").await;

    let resp = server.completion("comp.php", 1, 0).await;

    assert!(resp["error"].is_null(), "completion error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array() || result.get("items").is_some() || result.is_null(),
        "unexpected completion shape: {:?}",
        result
    );
}

#[tokio::test]
async fn completion_resolve_returns_item() {
    let mut server = TestServer::new().await;
    server
        .open(
            "cresolve.php",
            "<?php\nfunction resolveMe(): void {}\nresolveM\n",
        )
        .await;

    let comp = server.completion("cresolve.php", 2, 8).await;

    let items = match &comp["result"] {
        v if v.is_array() => v.as_array().unwrap().to_vec(),
        v if v["items"].is_array() => v["items"].as_array().unwrap().to_vec(),
        _ => vec![],
    };

    assert!(
        !items.is_empty(),
        "expected completions for 'resolveM' prefix, got: {:?}",
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
        "completionItem/resolve error: {:?}",
        resp
    );
    assert!(resp["result"].is_object(), "expected resolved item object");
    let detail = resp["result"]["detail"].as_str().unwrap_or("");
    assert!(
        detail.contains("resolveMe"),
        "resolved item must have detail populated with the function signature, got: {:?}",
        resp["result"]
    );
}
