//! Document link resolution: require/require_once paths and @link docblocks.

mod common;

use common::TestServer;
use serde_json::json;

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
