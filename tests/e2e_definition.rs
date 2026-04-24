//! Go-to-definition tests. Fixture DSL with `$0` cursor markers locates the
//! request position; multi-file fixtures cover cross-file resolution.

mod common;

use common::TestServer;
use serde_json::Value;

fn loc(result: &Value) -> Value {
    if result.is_array() {
        result[0].clone()
    } else {
        result.clone()
    }
}

#[tokio::test]
async fn definition_returns_location_for_function() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function greet(string $name): string { return $name; }
g$0reet('world');
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.definition(&c.path, c.line, c.character).await;

    assert!(resp["error"].is_null(), "definition error: {resp:?}");
    let l = loc(&resp["result"]);
    assert_eq!(l["uri"].as_str().unwrap(), server.uri(&c.path));
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
    let opened = server
        .open_fixture(
            r#"<?php
class Dog {}
$d = new D$0og();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.definition(&c.path, c.line, c.character).await;

    assert!(resp["error"].is_null(), "definition error: {resp:?}");
    let l = loc(&resp["result"]);
    assert_eq!(l["range"]["start"]["line"].as_u64().unwrap(), 1);
    assert_eq!(l["range"]["start"]["character"].as_u64().unwrap(), 6);
}

/// Cross-file goto-definition exercises `find_in_indexes`: the symbol is
/// defined in file A (open, so its FileIndex is populated) but the cursor
/// is in file B where the symbol is used.
#[tokio::test]
async fn definition_cross_file_uses_find_in_indexes() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"//- /greeter.php
<?php
class Greeter {}

//- /user.php
<?php
$g = new Gr$0eeter();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = server.definition(&c.path, c.line, c.character).await;

    assert!(resp["error"].is_null(), "definition error: {resp:?}");
    let l = loc(&resp["result"]);
    assert_eq!(
        l["uri"].as_str().unwrap(),
        server.uri("greeter.php"),
        "definition must point to greeter.php"
    );
    assert_eq!(
        l["range"]["start"]["line"].as_u64().unwrap(),
        1,
        "Greeter is declared on line 1 of greeter.php"
    );
}
