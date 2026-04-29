mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::{Value, json};

#[tokio::test]
async fn folding_ranges_cover_function_body() {
    let mut s = TestServer::new().await;
    let out = s
        .check_folding(
            r#"<?php
function f(): void {
    $a = 1;
    $b = 2;
    $c = 3;
}
"#,
        )
        .await;
    expect!["1..5 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_ranges_cover_class_and_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_folding(
            r#"<?php
class Folded {
    public function method(): void {
        // body
    }
}
"#,
        )
        .await;
    expect![[r#"
        1..5 region
        2..4 region"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_lens_for_function_with_reference() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
function lensed(): void {}
lensed();
"#,
        )
        .await;
    expect!["L1: 1 reference [editor.action.showReferences]"].assert_eq(&out);
}

#[tokio::test]
async fn code_lens_for_class_with_references() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class Widget {}
$w = new Widget();
"#,
        )
        .await;
    expect!["L1: 1 reference [editor.action.showReferences]"].assert_eq(&out);
}

fn render_resolved_lens(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let l = &resp["result"];
    let sl = l["range"]["start"]["line"].as_u64().unwrap_or(0);
    let title = l["command"]["title"].as_str().unwrap_or("<unresolved>");
    let cmd = l["command"]["command"].as_str().unwrap_or("");
    let data = if l.get("data").map(|d| !d.is_null()).unwrap_or(false) {
        format!(" data={}", l["data"])
    } else {
        String::new()
    };
    format!("L{sl}: {title} [{cmd}]{data}")
}

#[tokio::test]
async fn code_lens_resolve_round_trips_real_lens() {
    let mut server = TestServer::new().await;
    server
        .open("lens.php", "<?php\nfunction lensed(): void {}\nlensed();\n")
        .await;

    let lens = server.code_lens("lens.php").await["result"][0].clone();
    assert!(lens.is_object(), "expected at least one code lens");

    let resp = server.client().request("codeLens/resolve", lens).await;
    expect!["L1: 1 reference [editor.action.showReferences]"]
        .assert_eq(&render_resolved_lens(&resp));
}

#[tokio::test]
async fn code_lens_resolve_preserves_command_and_data() {
    let mut server = TestServer::new().await;
    let lens = json!({
        "range": {
            "start": { "line": 7, "character": 0 },
            "end":   { "line": 7, "character": 1 }
        },
        "command": {
            "title": "synthetic",
            "command": "noop",
            "arguments": [42]
        },
        "data": { "marker": "keep-me" }
    });

    let resp = server.client().request("codeLens/resolve", lens).await;
    expect![[r#"L7: synthetic [noop] data={"marker":"keep-me"}"#]]
        .assert_eq(&render_resolved_lens(&resp));
}

#[tokio::test]
async fn selection_range_expands_from_position() {
    let mut server = TestServer::new().await;
    server
        .open(
            "sel.php",
            "<?php\nfunction select(int $x): int { return $x + 1; }\n",
        )
        .await;

    let resp = server.selection_range("sel.php", vec![(1, 30)]).await;

    assert!(resp["error"].is_null(), "selectionRange error: {:?}", resp);
    let result = &resp["result"];
    assert!(
        result.is_array(),
        "selectionRange must return an array: {:?}",
        result
    );
    let items = result.as_array().unwrap();
    assert!(
        !items.is_empty(),
        "expected at least one selectionRange entry"
    );

    fn range_bounds(node: &Value) -> (u64, u64, u64, u64) {
        let r = &node["range"];
        (
            r["start"]["line"].as_u64().unwrap_or(0),
            r["start"]["character"].as_u64().unwrap_or(0),
            r["end"]["line"].as_u64().unwrap_or(0),
            r["end"]["character"].as_u64().unwrap_or(0),
        )
    }

    let mut current = &items[0];
    let (mut sl, mut sc, mut el, mut ec) = range_bounds(current);
    assert_ne!(ec, u32::MAX as u64, "end character must not be u32::MAX");

    let mut depth = 0usize;
    loop {
        let parent = &current["parent"];
        if !parent.is_object() {
            break;
        }
        let (psl, psc, pel, pec) = range_bounds(parent);
        assert!(
            (psl, psc) <= (sl, sc),
            "parent start {psl}:{psc} must be ≤ child start {sl}:{sc}"
        );
        assert!(
            (pel, pec) >= (el, ec),
            "parent end {pel}:{pec} must be ≥ child end {el}:{ec}"
        );
        (sl, sc, el, ec) = (psl, psc, pel, pec);
        current = parent;
        depth += 1;
    }
    assert!(
        depth >= 1,
        "expected at least one parent in the selection range chain"
    );
}
