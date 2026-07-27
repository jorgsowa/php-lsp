//! `workspace/partialResultToken` streaming for `textDocument/references`:
//! when a client supplies a token, owner-mentioning candidates are analyzed
//! and streamed via a `$/progress` notification before the (unchanged,
//! authoritative) response arrives. See `handle_references` in
//! `src/backend/handlers/navigation.rs`.

use super::*;
use serde_json::json;

const FIXTURE: &str = r#"//- /src/Owner.php
<?php
class Owner {
    public function pro$0cess(): void {}
}
$o = new Owner();
$o->process();

//- /src/Other.php
<?php
class Other {
    public function process(): void {}
}
$x = new Other();
$x->process();
"#;

fn references_params(
    uri: &str,
    line: u32,
    character: u32,
    partial_result_token: Option<&str>,
) -> serde_json::Value {
    let mut params = json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
        "context": { "includeDeclaration": true },
    });
    if let Some(token) = partial_result_token {
        params["partialResultToken"] = json!(token);
    }
    params
}

#[tokio::test]
async fn references_with_partial_result_token_streams_owner_mentioning_files_first() {
    let mut s = TestServer::new().await;
    let opened = s.open_fixture(FIXTURE).await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);

    let (resp, progress) = s
        .client()
        .request_capturing_notifications(
            "textDocument/references",
            references_params(&uri, c.line, c.character, Some("refs-token-1")),
            "$/progress",
        )
        .await;

    assert!(resp["error"].is_null(), "references errored: {resp:?}");
    assert!(
        !progress.is_empty(),
        "expected at least one $/progress notification, got none"
    );

    let final_locations = render_locations(&resp, &s.uri(""));
    for notif in &progress {
        assert_eq!(notif["params"]["token"], json!("refs-token-1"));
        let batch = json!({ "result": notif["params"]["value"] });
        let rendered_batch = render_locations(&batch, &s.uri(""));
        for line in rendered_batch.lines() {
            assert!(
                final_locations.lines().any(|l| l == line),
                "streamed location {line:?} missing from final response:\n{final_locations}"
            );
        }
    }

    expect_test::expect![[r#"
        src/Owner.php:2:20-2:27
        src/Owner.php:5:4-5:11"#]]
    .assert_eq(&final_locations);
}

#[tokio::test]
async fn references_without_partial_result_token_sends_no_progress() {
    let mut s = TestServer::new().await;
    let opened = s.open_fixture(FIXTURE).await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);

    let (resp, progress) = s
        .client()
        .request_capturing_notifications(
            "textDocument/references",
            references_params(&uri, c.line, c.character, None),
            "$/progress",
        )
        .await;

    assert!(resp["error"].is_null(), "references errored: {resp:?}");
    assert!(
        progress.is_empty(),
        "expected no $/progress notifications without a token, got {progress:?}"
    );
}

#[tokio::test]
async fn references_final_result_unchanged_by_partial_result_token() {
    let mut with_token = TestServer::new().await;
    let opened = with_token.open_fixture(FIXTURE).await;
    let c = opened.cursor().clone();
    let uri = with_token.uri(&c.path);
    let (resp_with_token, _) = with_token
        .client()
        .request_capturing_notifications(
            "textDocument/references",
            references_params(&uri, c.line, c.character, Some("refs-token-2")),
            "$/progress",
        )
        .await;

    let mut without_token = TestServer::new().await;
    let opened2 = without_token.open_fixture(FIXTURE).await;
    let c2 = opened2.cursor().clone();
    let uri2 = without_token.uri(&c2.path);
    let (resp_without_token, _) = without_token
        .client()
        .request_capturing_notifications(
            "textDocument/references",
            references_params(&uri2, c2.line, c2.character, None),
            "$/progress",
        )
        .await;

    assert_eq!(
        render_locations(&resp_with_token, &with_token.uri("")),
        render_locations(&resp_without_token, &without_token.uri(""))
    );
}
