mod common;

use common::TestServer;

#[tokio::test]
async fn declaration_returns_location_for_abstract_method() {
    let mut server = TestServer::new().await;
    server
        .open(
            "abs.php",
            "<?php\nabstract class Animal {\n    abstract public function speak(): string;\n}\nclass Cat extends Animal {\n    public function speak(): string { return 'meow'; }\n}\n",
        )
        .await;

    let resp = server.declaration("abs.php", 5, 20).await;

    assert!(resp["error"].is_null(), "declaration error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected a declaration location for concrete speak(), got null"
    );
    let loc = if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    };
    assert_eq!(loc["uri"].as_str().unwrap(), server.uri("abs.php"));
    assert_eq!(
        loc["range"]["start"]["line"].as_u64().unwrap(),
        2,
        "should point to the abstract declaration on line 2"
    );
    assert_eq!(
        loc["range"]["start"]["character"].as_u64().unwrap(),
        29,
        "should point to the method name, not the 'abstract' keyword"
    );
}
