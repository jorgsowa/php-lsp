//! Document link resolution: require/require_once paths and @link docblocks.

use super::*;

use expect_test::expect;
use serde_json::{Value, json};

fn render_document_links(result: &Value) -> String {
    let links = result.as_array().cloned().unwrap_or_default();
    if links.is_empty() {
        return "<no links>".to_owned();
    }
    links
        .iter()
        .map(|l| {
            let sl = l["range"]["start"]["line"].as_u64().unwrap_or(0);
            let sc = l["range"]["start"]["character"].as_u64().unwrap_or(0);
            let ec = l["range"]["end"]["character"].as_u64().unwrap_or(0);
            let target = l["target"].as_str().unwrap_or("<no target>");
            format!("{sl}:{sc}-{ec} target={target}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
    expect![[r#"
        1:14-33 target=file:///vendor/autoload.php
        2:9-23 target=file:///lib/helper.php"#]]
    .assert_eq(&render_document_links(&resp["result"]));
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
    expect!["1:10-35 target=https://php.net/array_map"]
        .assert_eq(&render_document_links(&resp["result"]));
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
    let mut server = TestServer::new().await;
    server.open("rng.php", "<?php\nrequire 'abc.php';\n").await;
    let link = server.document_link("rng.php").await["result"][0].clone();
    let resp = server.client().request("documentLink/resolve", link).await;
    expect!["1:9-16 target=abc.php"].assert_eq(&render_resolved_link(&resp, &server.uri("")));
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

#[tokio::test]
async fn document_link_resolve_external_url_roundtrips() {
    let mut server = TestServer::new().await;
    let link = json!({
        "range": {
            "start": { "line": 5, "character": 10 },
            "end": { "line": 5, "character": 30 }
        },
        "target": "https://docs.example.com/api"
    });

    let resp = server.client().request("documentLink/resolve", link).await;
    expect!["5:10-30 target=https://docs.example.com/api"]
        .assert_eq(&render_resolved_link(&resp, "file://"));
}

#[tokio::test]
async fn document_link_resolve_link_without_target_field() {
    let mut server = TestServer::new().await;
    let link = json!({
        "range": {
            "start": { "line": 0, "character": 5 },
            "end": { "line": 0, "character": 15 }
        }
    });

    let resp = server.client().request("documentLink/resolve", link).await;
    assert!(resp["error"].is_null(), "no error for link without target");
    let result = &resp["result"];
    assert!(
        result["range"].is_object(),
        "range should be preserved: {result:?}"
    );
    // target may be null or absent — document_link_resolve is a passthrough
    assert!(result["target"].is_null() || result.get("target").is_none());
}

#[tokio::test]
async fn document_link_resolve_with_null_target() {
    let mut server = TestServer::new().await;
    let link = json!({
        "range": {
            "start": { "line": 2, "character": 5 },
            "end": { "line": 2, "character": 15 }
        },
        "target": null
    });

    let resp = server.client().request("documentLink/resolve", link).await;
    assert!(resp["error"].is_null());
    let result = &resp["result"];
    assert!(result["target"].is_null(), "null target must be preserved");
}

#[tokio::test]
async fn document_link_resolve_preserves_data_field() {
    let mut server = TestServer::new().await;
    let link = json!({
        "range": {
            "start": { "line": 0, "character": 5 },
            "end": { "line": 0, "character": 15 }
        },
        "target": "file://example.php",
        "tooltip": "A file link",
        "data": { "linkId": "456", "metadata": { "resolved": false } }
    });

    let resp = server
        .client()
        .request("documentLink/resolve", link.clone())
        .await;
    assert!(resp["error"].is_null(), "error: {:?}", resp.get("error"));
    let result = &resp["result"];
    assert_eq!(
        result["data"], link["data"],
        "data field must be preserved exactly"
    );
}

#[tokio::test]
async fn document_link_resolve_is_idempotent() {
    let mut server = TestServer::new().await;
    server
        .open("require.php", "<?php\nrequire 'helpers.php';\n")
        .await;

    let links = server.document_link("require.php").await["result"]
        .as_array()
        .cloned()
        .expect("expected links");
    let first_link = links[0].clone();

    let resolved_once = server
        .client()
        .request("documentLink/resolve", first_link.clone())
        .await;
    let resolved_twice = server
        .client()
        .request("documentLink/resolve", resolved_once["result"].clone())
        .await;

    assert_eq!(
        resolved_once["result"], resolved_twice["result"],
        "calling resolve twice must return identical results (idempotent)"
    );
}
