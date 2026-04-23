mod common;

use common::TestServer;

#[tokio::test]
async fn type_definition_for_typed_variable() {
    let mut server = TestServer::new().await;
    server
        .open(
            "typedef.php",
            "<?php\nclass Point { public int $x; public int $y; }\n$p = new Point();\n$p->x;\n",
        )
        .await;

    let resp = server.type_definition("typedef.php", 3, 1).await;

    assert!(resp["error"].is_null(), "typeDefinition error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected typeDefinition to resolve $p to Point, got null"
    );
    let loc = if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    };
    assert_eq!(
        loc["range"]["start"]["line"].as_u64().unwrap(),
        1,
        "type definition should point to Point class on line 1"
    );
    assert_eq!(
        loc["range"]["start"]["character"].as_u64().unwrap(),
        6,
        "type definition should point to the class name, not the 'class' keyword"
    );
}
