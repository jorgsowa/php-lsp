//! Document link resolution: require/require_once paths and @link docblocks.

mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::{Value, json};

#[tokio::test]
async fn document_link_multiple_requires_produce_multiple_links() {
    let mut server = TestServer::new().await;
    server
        .open(
            "multi.php",
            "<?php\nrequire_once 'vendor/autoload.php';\nrequire 'lib/helper.php';\n",
        )
        .await;
    let resp = server.document_link("multi.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");
    let links = resp["result"].as_array().expect("expected array of links");
    assert_eq!(links.len(), 2, "expected 2 links, got: {links:?}");
}

#[tokio::test]
async fn document_link_docblock_at_link_produces_http_link() {
    let mut server = TestServer::new().await;
    server
        .open(
            "doclink.php",
            "<?php\n/** @link https://php.net/array_map */\nfunction f() {}\n",
        )
        .await;
    let resp = server.document_link("doclink.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");
    let links = resp["result"].as_array().expect("array");
    assert_eq!(links.len(), 1, "expected 1 link: {links:?}");
    assert_eq!(
        links[0]["target"].as_str().unwrap_or(""),
        "https://php.net/array_map",
        "link target mismatch: {:?}",
        links[0]
    );
}

#[tokio::test]
async fn document_link_at_see_class_ref_produces_no_link() {
    let mut server = TestServer::new().await;
    server
        .open(
            "nosee.php",
            "<?php\n/** @see SomeClass::method */\nfunction g() {}\n",
        )
        .await;
    let resp = server.document_link("nosee.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");
    assert!(
        resp["result"].is_null()
            || resp["result"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(false),
        "expected no link for non-HTTP @see, got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn document_link_plain_file_returns_null() {
    let mut server = TestServer::new().await;
    server.open("empty.php", "<?php\n$x = 1;\n").await;
    let resp = server.document_link("empty.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");
    assert!(
        resp["result"].is_null(),
        "expected null for file with no links: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn document_link_require_target_is_file_uri() {
    let mut server = TestServer::new().await;
    server
        .open("req.php", "<?php\nrequire 'helpers/utils.php';\n")
        .await;
    let resp = server.document_link("req.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");
    let links = resp["result"].as_array().expect("array");
    assert_eq!(links.len(), 1);
    let target = links[0]["target"].as_str().unwrap_or("");
    assert!(
        target.starts_with("file://") && target.contains("helpers/utils.php"),
        "expected file:// URI with path, got: {target:?}"
    );
}

#[tokio::test]
async fn document_link_range_is_inside_quotes() {
    // Source line 1: `require 'abc.php';`
    // "require " = 8 chars, opening quote `'` at col 8, content starts at col 9.
    // "abc.php" = 7 chars, so end character = 9 + 7 = 16.
    let mut server = TestServer::new().await;
    server.open("rng.php", "<?php\nrequire 'abc.php';\n").await;
    let resp = server.document_link("rng.php").await;
    assert!(resp["error"].is_null());
    let links = resp["result"].as_array().expect("array");
    assert_eq!(links.len(), 1);
    let range = &links[0]["range"];
    assert_eq!(range["start"]["line"], json!(1), "link is on line 1");
    assert_eq!(
        range["start"]["character"],
        json!(9),
        "path starts at char 9 (after \"require '\")"
    );
    assert_eq!(
        range["end"]["character"],
        json!(16),
        "path ends at char 16 (9 + len('abc.php') = 16)"
    );
}

fn render_resolved_link(resp: &Value, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let l = &resp["result"];
    let sl = l["range"]["start"]["line"].as_u64().unwrap_or(0);
    let sc = l["range"]["start"]["character"].as_u64().unwrap_or(0);
    let ec = l["range"]["end"]["character"].as_u64().unwrap_or(0);
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let target = l["target"].as_str().unwrap_or("");
    let target = target.strip_prefix(&prefix).unwrap_or(target).to_owned();
    let tooltip = l["tooltip"]
        .as_str()
        .map(|t| format!(" tooltip={t}"))
        .unwrap_or_default();
    let data = if l.get("data").map(|d| !d.is_null()).unwrap_or(false) {
        format!(" data={}", l["data"])
    } else {
        String::new()
    };
    format!("{sl}:{sc}-{ec} target={target}{tooltip}{data}")
}

#[tokio::test]
async fn document_link_resolve_round_trips_real_link() {
    let mut server = TestServer::new().await;
    server
        .open("res.php", "<?php\nrequire 'helpers/utils.php';\n")
        .await;

    let link = server.document_link("res.php").await["result"][0].clone();
    assert!(link.is_object(), "expected at least one document link");

    let root = server.uri("");
    let resp = server.client().request("documentLink/resolve", link).await;
    expect!["1:9-26 target=helpers/utils.php"].assert_eq(&render_resolved_link(&resp, &root));
}

#[tokio::test]
async fn document_link_resolve_preserves_target_tooltip_and_data() {
    let mut server = TestServer::new().await;
    let link = json!({
        "range": {
            "start": { "line": 3, "character": 5 },
            "end":   { "line": 3, "character": 20 }
        },
        "target": "https://example.test/some/path",
        "tooltip": "synthetic link",
        "data": { "marker": "preserve" }
    });

    let resp = server.client().request("documentLink/resolve", link).await;
    expect![[
        r#"3:5-20 target=https://example.test/some/path tooltip=synthetic link data={"marker":"preserve"}"#
    ]]
    .assert_eq(&render_resolved_link(&resp, "ignored://"));
}
