mod common;

use common::TestServer;

#[tokio::test]
async fn declaration_returns_location_for_abstract_method() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
abstract class Animal {
    abstract public function speak(): string;
}
class Cat extends Animal {
    public function sp$0eak(): string { return 'meow'; }
}
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.declaration(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "declaration error: {resp:?}");
    let result = &resp["result"];
    assert!(!result.is_null(), "expected a declaration location");
    let loc = if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    };
    assert_eq!(loc["uri"].as_str().unwrap(), server.uri(&c.path));
    assert_eq!(
        loc["range"]["start"]["line"].as_u64().unwrap(),
        2,
        "should point to the abstract declaration"
    );
    assert_eq!(
        loc["range"]["start"]["character"].as_u64().unwrap(),
        29,
        "should point to the method name, not the 'abstract' keyword"
    );
}
