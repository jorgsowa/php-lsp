mod common;

use common::TestServer;

#[tokio::test]
async fn implementation_finds_concrete_class() {
    let mut server = TestServer::new().await;
    server
        .open(
            "impl.php",
            "<?php\ninterface Drawable {\n    public function draw(): void;\n}\nclass Circle implements Drawable {\n    public function draw(): void {}\n}\n",
        )
        .await;

    let resp = server.implementation("impl.php", 1, 10).await;

    assert!(resp["error"].is_null(), "implementation error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "implementation must return an array: {:?}",
        result
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
