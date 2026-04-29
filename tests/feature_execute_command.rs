//! Tests for `workspace/executeCommand` / `php-lsp.runTest`.
//!
//! The handler spawns a PHPUnit process and posts `window/showMessageRequest`
//! back to the client.  Tests use `TestClient::expect_server_request` and
//! `reply_to_server_request` to capture and drive those server→client
//! interactions over the real wire protocol.

mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::{Value, json};

// ---------- fake phpunit ----------

/// Create `vendor/bin/phpunit` in `root` as a shell script that prints
/// `stdout` and exits with `exit_code`.  The output is written to a sibling
/// file and `cat`-ed by the script so that no shell-quoting of arbitrary
/// strings is needed.
#[cfg(unix)]
fn write_fake_phpunit(root: &std::path::Path, exit_code: i32, stdout: &str) {
    let dir = root.join("vendor/bin");
    std::fs::create_dir_all(&dir).unwrap();
    let out_file = dir.join("phpunit.out");
    std::fs::write(&out_file, stdout).unwrap();
    let script_content = format!(
        "#!/bin/sh\ncat \"{}\"\nprintf '\\n'\nexit {exit_code}\n",
        out_file.display()
    );
    let script = dir.join("phpunit");
    std::fs::write(&script, script_content).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------- request helper ----------

async fn send_run_test(
    s: &mut TestServer,
    file_uri: Option<&str>,
    filter: &str,
) -> serde_json::Value {
    s.client()
        .request(
            "workspace/executeCommand",
            json!({
                "command": "php-lsp.runTest",
                "arguments": [
                    file_uri.map(|u| json!(u)).unwrap_or(json!(null)),
                    json!(filter),
                ],
            }),
        )
        .await
}

// ---------- render helpers (local to this feature) ----------

fn message_type_name(t: u64) -> &'static str {
    match t {
        1 => "ERROR",
        2 => "WARNING",
        3 => "INFO",
        4 => "LOG",
        _ => "?",
    }
}

/// Render `window/showMessageRequest` params as a stable snapshot string.
/// Normalizes platform-specific OS error tails so spawn-failure snapshots
/// are identical on Linux, macOS, and Windows CI.
fn render_show_message_request(params: &Value) -> String {
    let t = message_type_name(params["type"].as_u64().unwrap_or(0));
    let raw = params["message"].as_str().unwrap_or("");
    let sentinel = "failed to spawn phpunit — ";
    let msg = if let Some(pos) = raw.find(sentinel) {
        format!("{}<os error>", &raw[..pos + sentinel.len()])
    } else {
        raw.to_owned()
    };
    let actions: Vec<&str> = params["actions"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["title"].as_str()).collect())
        .unwrap_or_default();
    if actions.is_empty() {
        format!("{t}: {msg}")
    } else {
        format!("{t}: {msg}\nactions: {}", actions.join(", "))
    }
}

/// Render `window/showMessage` notification params.
fn render_show_message(params: &Value) -> String {
    let t = message_type_name(params["type"].as_u64().unwrap_or(0));
    let msg = params["message"].as_str().unwrap_or("");
    format!("{t}: {msg}")
}

/// Render `window/showDocument` params.  The URI is stripped of the workspace
/// root prefix so tempdir paths don't leak into snapshots.
fn render_show_document(params: &Value, root_uri: &str) -> String {
    let uri = params["uri"].as_str().unwrap_or("?");
    let prefix = format!("{}/", root_uri.trim_end_matches('/'));
    let short = uri.strip_prefix(&prefix).unwrap_or(uri);
    let take_focus = params["takeFocus"].as_bool().unwrap_or(false);
    let mut out = format!("uri: {short}\ntakeFocus: {take_focus}");
    if let Some(e) = params["external"].as_bool() {
        out.push_str(&format!("\nexternal: {e}"));
    }
    out
}

// ---------- tests ----------

#[tokio::test]
async fn unknown_command_returns_null() {
    let mut s = TestServer::new().await;
    let resp = s
        .client()
        .request(
            "workspace/executeCommand",
            json!({ "command": "unknown.command", "arguments": [] }),
        )
        .await;
    expect!["null"].assert_eq(&resp["result"].to_string());
}

/// When `vendor/bin/phpunit` is not present the handler posts an error message.
/// Without a file URI the "Open File" action is not offered.
#[tokio::test]
async fn run_test_phpunit_not_found_reports_error() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;

    send_run_test(&mut s, None, "FooTest::testSomething").await;

    let (_id, params) = s
        .client()
        .expect_server_request("window/showMessageRequest")
        .await;

    expect![[r#"
        ERROR: php-lsp.runTest: failed to spawn phpunit — <os error>
        actions: Run Again"#]]
    .assert_eq(&render_show_message_request(&params));
}

/// When a file URI is supplied and phpunit cannot be found, "Open File" is
/// also offered alongside "Run Again".
#[tokio::test]
async fn run_test_phpunit_not_found_with_file_uri_offers_open_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    let file_uri = s.uri("FooTest.php");

    send_run_test(&mut s, Some(&file_uri), "FooTest::testSomething").await;

    let (_id, params) = s
        .client()
        .expect_server_request("window/showMessageRequest")
        .await;

    expect![[r#"
        ERROR: php-lsp.runTest: failed to spawn phpunit — <os error>
        actions: Run Again, Open File"#]]
    .assert_eq(&render_show_message_request(&params));
}

/// A passing test suite produces an INFO message whose text starts with "✓".
#[cfg(unix)]
#[tokio::test]
async fn run_test_phpunit_success_shows_info_message() {
    let tmp = tempfile::tempdir().unwrap();
    write_fake_phpunit(tmp.path(), 0, "OK (1 test, 1 assertion)");
    let mut s = TestServer::with_root(tmp.path()).await;

    send_run_test(&mut s, None, "PassTest::testPass").await;

    let (_id, params) = s
        .client()
        .expect_server_request("window/showMessageRequest")
        .await;

    expect![[r#"
        INFO: ✓ PassTest::testPass: OK (1 test, 1 assertion)
        actions: Run Again"#]]
    .assert_eq(&render_show_message_request(&params));
}

/// A failing test suite produces an ERROR message with "✗" and additionally
/// offers "Open File" when a URI was provided.
#[cfg(unix)]
#[tokio::test]
async fn run_test_phpunit_failure_shows_error_with_open_file() {
    let tmp = tempfile::tempdir().unwrap();
    write_fake_phpunit(tmp.path(), 1, "FAILURES!");
    let mut s = TestServer::with_root(tmp.path()).await;
    let file_uri = s.uri("FailTest.php");

    send_run_test(&mut s, Some(&file_uri), "FailTest::testFail").await;

    let (_id, params) = s
        .client()
        .expect_server_request("window/showMessageRequest")
        .await;

    expect![[r#"
        ERROR: ✗ FailTest::testFail: FAILURES!
        actions: Run Again, Open File"#]]
    .assert_eq(&render_show_message_request(&params));
}

/// Choosing "Run Again" re-runs phpunit and shows the result as a plain
/// `window/showMessage` notification (not another request).
#[cfg(unix)]
#[tokio::test]
async fn run_test_run_again_reruns_test() {
    let tmp = tempfile::tempdir().unwrap();
    write_fake_phpunit(tmp.path(), 0, "OK (1 test, 1 assertion)");
    let mut s = TestServer::with_root(tmp.path()).await;

    send_run_test(&mut s, None, "PassTest::testPass").await;

    let (req_id, _params) = s
        .client()
        .expect_server_request("window/showMessageRequest")
        .await;

    s.client()
        .reply_to_server_request(req_id, json!({ "title": "Run Again" }))
        .await;

    // Re-run result arrives as a plain notification, not a request.
    let notif = s.client().read_notification("window/showMessage").await;
    expect!["INFO: ✓ PassTest::testPass: OK (1 test, 1 assertion)"]
        .assert_eq(&render_show_message(&notif["params"]));
}

/// Choosing "Open File" triggers a `window/showDocument` server request
/// pointing at the URI that was originally provided.
#[cfg(unix)]
#[tokio::test]
async fn run_test_open_file_shows_document() {
    let tmp = tempfile::tempdir().unwrap();
    write_fake_phpunit(tmp.path(), 1, "FAILURES!");
    let mut s = TestServer::with_root(tmp.path()).await;
    let file_uri = s.uri("FailTest.php");

    send_run_test(&mut s, Some(&file_uri), "FailTest::testFail").await;

    let (req_id, _params) = s
        .client()
        .expect_server_request("window/showMessageRequest")
        .await;

    s.client()
        .reply_to_server_request(req_id, json!({ "title": "Open File" }))
        .await;

    let (doc_id, doc_params) = s
        .client()
        .expect_server_request("window/showDocument")
        .await;

    expect![[r#"
        uri: FailTest.php
        takeFocus: true
        external: false"#]]
    .assert_eq(&render_show_document(&doc_params, &s.uri("")));

    // Ack so the spawned task can exit cleanly.
    s.client()
        .reply_to_server_request(doc_id, json!({ "success": true }))
        .await;
}

/// On success (exit 0), "Open File" is NOT offered even when a file URI was
/// provided — the conditional is `!success && file_uri.is_some()`.
#[cfg(unix)]
#[tokio::test]
async fn run_test_success_with_file_uri_does_not_offer_open_file() {
    let tmp = tempfile::tempdir().unwrap();
    write_fake_phpunit(tmp.path(), 0, "OK (1 test, 1 assertion)");
    let mut s = TestServer::with_root(tmp.path()).await;
    let file_uri = s.uri("PassTest.php");

    send_run_test(&mut s, Some(&file_uri), "PassTest::testPass").await;

    let (_id, params) = s
        .client()
        .expect_server_request("window/showMessageRequest")
        .await;

    expect![[r#"
        INFO: ✓ PassTest::testPass: OK (1 test, 1 assertion)
        actions: Run Again"#]]
    .assert_eq(&render_show_message_request(&params));
}
