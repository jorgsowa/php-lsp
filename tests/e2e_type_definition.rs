mod common;

use common::TestServer;

#[tokio::test]
async fn type_definition_for_typed_variable() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
class Point { public int $x; public int $y; }
$p = new Point();
$$0p->x;
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.type_definition(&c.path, c.line, c.character).await;
    assert!(resp["error"].is_null(), "typeDefinition error: {resp:?}");
    let result = &resp["result"];
    assert!(!result.is_null(), "expected typeDefinition result");
    let loc = if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    };
    assert_eq!(
        loc["range"]["start"]["line"].as_u64().unwrap(),
        1,
        "type definition should point to the Point class line"
    );
    assert_eq!(
        loc["range"]["start"]["character"].as_u64().unwrap(),
        6,
        "type definition should point to the class name, not the 'class' keyword"
    );
}
