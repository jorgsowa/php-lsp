//! Document + workspace symbol coverage.

mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::json;

#[tokio::test]
async fn document_symbols_outline() {
    let mut s = TestServer::new().await;
    let out = s
        .check_document_symbols(
            r#"<?php
class Greeter {
    public function hello(): string { return 'hi'; }
    public function bye(): void {}
}
function top_level(): void {}
"#,
        )
        .await;
    expect![[r#"
        Class Greeter @L1
          Method hello @L2
          Method bye @L3
        Function top_level @L5"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_nested_enum() {
    let mut s = TestServer::new().await;
    let out = s
        .check_document_symbols(
            r#"<?php
enum Status {
    case Active;
    case Inactive;
}
"#,
        )
        .await;
    expect![[r#"
        Enum Status @L1
          EnumMember Active @L2
          EnumMember Inactive @L3"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_symbols_interface() {
    let mut s = TestServer::new().await;
    let out = s
        .check_document_symbols(
            r#"<?php
interface Writable {
    public function write(): void;
}
"#,
        )
        .await;
    expect![[r#"
        Interface Writable @L1
          Method write @L2"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn workspace_symbols_finds_class_by_query() {
    let mut s = TestServer::new().await;
    let out = s
        .check_workspace_symbols(
            r#"<?php
class MagicRegistry {}
function abracadabra(): void {}
"#,
            "MagicReg",
        )
        .await;
    expect!["Class       MagicRegistry @ main.php:1"].assert_eq(&out);
}

/// Workspace symbol search must find `User` by short name even though the FQN
/// is `App\Model\User`. Matches on exact name + class kind + correct file URI.
#[tokio::test]
async fn workspace_symbol_finds_class_by_short_name() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;
    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().expect("symbols array");
    let matched = symbols.iter().find(|s| {
        s["name"].as_str() == Some("User")
            && s["kind"].as_u64() == Some(5)
            && s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Model/User.php"))
                .unwrap_or(false)
    });
    assert!(
        matched.is_some(),
        "expected exact class `User` in src/Model/User.php, got: {symbols:?}"
    );
}

// --- workspaceSymbol/resolve ---

#[tokio::test]
async fn symbol_resolve_fills_range_for_open_file() {
    let mut server = TestServer::new().await;
    server
        .open("resolve.php", "<?php\nclass Resolvable {}\n")
        .await;
    let uri = server.uri("resolve.php");

    let symbol = json!({
        "name": "Resolvable",
        "kind": 5,
        "location": { "uri": uri },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;

    assert!(resp["error"].is_null(), "error: {resp:?}");
    let loc = &resp["result"]["location"];
    assert!(
        loc["range"].is_object(),
        "expected range to be filled in for open file: {loc:?}"
    );
    // `class Resolvable` is on line 1; the name starts at char 6 (after "class ").
    assert_eq!(
        loc["range"]["start"]["line"],
        json!(1),
        "wrong line: {loc:?}"
    );
    assert_eq!(
        loc["range"]["start"]["character"],
        json!(6),
        "wrong char: {loc:?}"
    );
}

#[tokio::test]
async fn symbol_resolve_unchanged_for_closed_file() {
    let mut server = TestServer::new().await;

    let symbol = json!({
        "name": "ClosedClass",
        "kind": 5,
        "location": { "uri": "file:///nonexistent_closed.php" },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;

    assert!(resp["error"].is_null(), "error: {resp:?}");
    let loc = &resp["result"]["location"];
    assert!(
        !loc.as_object()
            .map(|o| o.contains_key("range"))
            .unwrap_or(false),
        "expected URI-only location for closed file (no range key): {loc:?}"
    );
}

#[tokio::test]
async fn symbol_resolve_passthrough_for_already_resolved_location() {
    let mut server = TestServer::new().await;
    server
        .open("passthrough.php", "<?php\nfunction alreadyResolved() {}\n")
        .await;
    let uri = server.uri("passthrough.php");

    let symbol = json!({
        "name": "alreadyResolved",
        "kind": 12,
        "location": {
            "uri": uri,
            "range": {
                "start": { "line": 1, "character": 9 },
                "end":   { "line": 1, "character": 24 },
            },
        },
    });
    let resp = server.workspace_symbol_resolve(symbol).await;

    assert!(resp["error"].is_null(), "error: {resp:?}");
    let range = &resp["result"]["location"]["range"];
    assert_eq!(range["start"]["line"], json!(1));
    assert_eq!(range["start"]["character"], json!(9));
    assert_eq!(range["end"]["line"], json!(1));
    assert_eq!(range["end"]["character"], json!(24));
}
