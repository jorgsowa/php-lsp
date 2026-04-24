//! Code-action tests. Uses the two-`$0` selection DSL so the region
//! requested is literally visible in the fixture.

mod common;

use common::TestServer;
use serde_json::Value;

fn titles(resp: &Value) -> Vec<String> {
    resp["result"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a["title"].as_str().map(str::to_owned))
        .collect()
}

async fn titles_at_range(server: &mut TestServer, fixture: &str) -> Vec<String> {
    let opened = server.open_fixture(fixture).await;
    let r = opened.range().clone();
    let resp = server.code_action_at(&r).await;
    assert!(resp["error"].is_null(), "codeAction error: {resp:?}");
    titles(&resp)
}

#[tokio::test]
async fn code_action_phpdoc_offered_for_undocumented_function() {
    let mut server = TestServer::new().await;
    let t = titles_at_range(
        &mut server,
        r#"<?php
function $0noDoc$0(int $x): int { return $x; }
"#,
    )
    .await;
    assert!(
        t.iter().any(|s| s.to_lowercase().contains("phpdoc")),
        "expected a PHPDoc action: {t:?}"
    );
}

#[tokio::test]
async fn code_action_extract_variable_offered_on_expression() {
    let mut server = TestServer::new().await;
    let t = titles_at_range(
        &mut server,
        r#"<?php
$result = $01 + 2$0;
"#,
    )
    .await;
    assert!(
        t.iter().any(|s| s.to_lowercase().contains("extract")),
        "expected an Extract action: {t:?}"
    );
}

#[tokio::test]
async fn code_action_generate_constructor_offered_for_class() {
    let mut server = TestServer::new().await;
    let t = titles_at_range(
        &mut server,
        r#"<?php
class $0Point$0 {
    public int $x;
    public int $y;
}
"#,
    )
    .await;
    assert!(
        t.iter().any(|s| s.to_lowercase().contains("constructor")),
        "expected a Generate constructor action: {t:?}"
    );
}

#[tokio::test]
async fn code_action_implement_missing_offered() {
    let mut server = TestServer::new().await;
    // Caret inside the empty class body: use a zero-width selection at
    // column 0 of the `}` line by repeating the marker.
    let t = titles_at_range(
        &mut server,
        r#"<?php
interface Greetable {
    public function greet(): string;
}
class Hello implements Greetable {
$0$0}
"#,
    )
    .await;
    assert!(
        t.iter().any(|s| s.to_lowercase().contains("implement")),
        "expected an Implement action: {t:?}"
    );
}

#[tokio::test]
async fn code_action_add_return_type_offered() {
    let mut server = TestServer::new().await;
    let t = titles_at_range(
        &mut server,
        r#"<?php
function $0noReturn$0() { return 42; }
"#,
    )
    .await;
    assert!(
        t.iter().any(|s| s.to_lowercase().contains("return type")),
        "expected an Add return type action: {t:?}"
    );
}
