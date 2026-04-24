mod common;

use common::TestServer;

#[tokio::test]
async fn implementation_finds_concrete_class() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
interface Dr$0awable {
    public function draw(): void;
}
class Circle implements Drawable {
    public function draw(): void {}
}
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.implementation(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "implementation error: {resp:?}");
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "implementation must return an array: {result:?}"
    );
    let locs = result.as_array().unwrap();
    assert!(
        !locs.is_empty(),
        "expected at least one implementation (Circle)"
    );
    let circle = locs
        .iter()
        .find(|l| l["range"]["start"]["line"].as_u64() == Some(4))
        .expect("expected an implementation result on line 4 (class Circle)");
    assert_eq!(
        circle["range"]["start"]["character"].as_u64().unwrap(),
        6,
        "Circle class name should start at char 6, not the 'class' keyword"
    );
}
