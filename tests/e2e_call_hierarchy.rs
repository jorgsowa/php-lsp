mod common;

use common::TestServer;

#[tokio::test]
async fn call_hierarchy_prepare_returns_item() {
    let mut server = TestServer::new().await;
    server
        .open("ch.php", "<?php\nfunction callee(): void {}\ncallee();\n")
        .await;

    let resp = server.prepare_call_hierarchy("ch.php", 1, 9).await;

    assert!(
        resp["error"].is_null(),
        "prepareCallHierarchy error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "expected array result, got: {:?}",
        result
    );
    let items = result.as_array().unwrap();
    assert!(!items.is_empty(), "expected at least one CallHierarchyItem");
    assert_eq!(
        items[0]["name"].as_str().unwrap_or(""),
        "callee",
        "expected item name to be 'callee', got: {:?}",
        items[0]
    );
}

#[tokio::test]
async fn call_hierarchy_incoming_calls_finds_caller() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ch_in.php",
            "<?php\nfunction callee(): void {}\nfunction caller(): void { callee(); }\n",
        )
        .await;

    let prep = server.prepare_call_hierarchy("ch_in.php", 1, 9).await;
    let item = prep["result"][0].clone();
    assert!(item.is_object(), "need a prepared item to continue");

    let resp = server.incoming_calls(item).await;

    assert!(resp["error"].is_null(), "incomingCalls error: {:?}", resp);
    let calls = resp["result"].as_array().expect("expected array");
    assert!(!calls.is_empty(), "expected at least one incoming call");
    assert!(
        calls
            .iter()
            .any(|c| c["from"]["name"].as_str() == Some("caller")),
        "expected 'caller' as incoming caller, got: {:?}",
        calls
    );
}

#[tokio::test]
async fn call_hierarchy_outgoing_calls_finds_callee() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ch_out.php",
            "<?php\nfunction inner(): void {}\nfunction outer(): void { inner(); }\n",
        )
        .await;

    let prep = server.prepare_call_hierarchy("ch_out.php", 2, 9).await;
    let item = prep["result"][0].clone();
    assert!(item.is_object(), "need a prepared item to continue");

    let resp = server.outgoing_calls(item).await;

    assert!(resp["error"].is_null(), "outgoingCalls error: {:?}", resp);
    let calls = resp["result"].as_array().expect("expected array");
    assert!(!calls.is_empty(), "expected at least one outgoing call");
    assert!(
        calls
            .iter()
            .any(|c| c["to"]["name"].as_str() == Some("inner")),
        "expected 'inner' as outgoing callee, got: {:?}",
        calls
    );
}
