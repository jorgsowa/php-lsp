mod common;

use common::TestServer;

fn loc(result: &serde_json::Value) -> serde_json::Value {
    if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    }
}

#[tokio::test]
async fn definition_returns_location_for_function() {
    let mut server = TestServer::new().await;
    server
        .open(
            "def.php",
            "<?php\nfunction greet(string $name): string { return $name; }\ngreet('world');\n",
        )
        .await;

    let resp = server.definition("def.php", 2, 1).await;

    assert!(resp["error"].is_null(), "definition error: {:?}", resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "expected a definition location");
    let l = loc(result);
    assert_eq!(l["uri"].as_str().unwrap(), server.uri("def.php"));
    assert_eq!(l["range"]["start"]["line"].as_u64().unwrap(), 1);
    assert_eq!(
        l["range"]["start"]["character"].as_u64().unwrap(),
        9,
        "should point to the function name, not the 'function' keyword"
    );
}

#[tokio::test]
async fn definition_for_class_returns_location() {
    let mut server = TestServer::new().await;
    server
        .open("cls.php", "<?php\nclass Dog {}\n$d = new Dog();\n")
        .await;

    let resp = server.definition("cls.php", 2, 9).await;

    assert!(resp["error"].is_null(), "definition error: {:?}", resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "expected a location for class Dog");
    let l = loc(result);
    assert_eq!(l["range"]["start"]["line"].as_u64().unwrap(), 1);
    assert_eq!(l["range"]["start"]["character"].as_u64().unwrap(), 6);
}
