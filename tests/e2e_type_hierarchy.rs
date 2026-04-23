mod common;

use common::TestServer;

#[tokio::test]
async fn type_hierarchy_prepare_returns_item() {
    let mut server = TestServer::new().await;
    server.open("th.php", "<?php\nclass MyClass {}\n").await;

    let resp = server.prepare_type_hierarchy("th.php", 1, 6).await;

    assert!(
        resp["error"].is_null(),
        "prepareTypeHierarchy error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(result.is_array(), "expected array, got: {:?}", result);
    let items = result.as_array().unwrap();
    assert!(!items.is_empty(), "expected at least one TypeHierarchyItem");
    assert_eq!(
        items[0]["name"].as_str().unwrap_or(""),
        "MyClass",
        "expected item name 'MyClass', got: {:?}",
        items[0]
    );
}

#[tokio::test]
async fn type_hierarchy_supertypes_finds_parent() {
    let mut server = TestServer::new().await;
    server
        .open(
            "th_super.php",
            "<?php\nclass ParentClass {}\nclass ChildClass extends ParentClass {}\n",
        )
        .await;

    let prep = server.prepare_type_hierarchy("th_super.php", 2, 6).await;
    let item = prep["result"][0].clone();
    assert!(item.is_object(), "need a prepared item to continue");

    let resp = server.supertypes(item).await;

    assert!(resp["error"].is_null(), "supertypes error: {:?}", resp);
    let types = resp["result"].as_array().expect("expected array");
    assert!(!types.is_empty(), "expected parent in supertypes");
    assert!(
        types
            .iter()
            .any(|t| t["name"].as_str() == Some("ParentClass")),
        "expected ParentClass in supertypes, got: {:?}",
        types
    );
}

#[tokio::test]
async fn type_hierarchy_subtypes_finds_child() {
    let mut server = TestServer::new().await;
    server
        .open(
            "th_sub.php",
            "<?php\ninterface Runnable {}\nclass Runner implements Runnable {}\n",
        )
        .await;

    let prep = server.prepare_type_hierarchy("th_sub.php", 1, 10).await;
    let item = prep["result"][0].clone();
    assert!(item.is_object(), "need a prepared item to continue");

    let resp = server.subtypes(item).await;

    assert!(resp["error"].is_null(), "subtypes error: {:?}", resp);
    let types = resp["result"].as_array().expect("expected array");
    assert!(!types.is_empty(), "expected child in subtypes");
    assert!(
        types.iter().any(|t| t["name"].as_str() == Some("Runner")),
        "expected Runner in subtypes, got: {:?}",
        types
    );
}
