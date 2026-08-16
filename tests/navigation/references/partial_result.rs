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

const PRIVATE_METHOD_FIXTURE: &str = r#"//- /src/Owner.php
<?php
class Owner {
    private function pro$0cess(): void {}

    public function run(): void {
        $this->process();
    }
}
"#;

/// A `private` method's candidate scope is already narrowed to the single
/// declaring file by `reference_candidate_files` — there is no "rest of the
/// workspace" left to prioritize against, so the priority-partition's
/// owner-mention text scan has nothing to contribute and must be skipped
/// entirely (no `$/progress` sent), even though a token was supplied. The
/// authoritative response must still be correct.
#[tokio::test]
async fn references_with_partial_result_token_skips_scan_for_narrowed_scope() {
    let mut s = TestServer::new().await;
    let opened = s.open_fixture(PRIVATE_METHOD_FIXTURE).await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);

    let (resp, progress) = s
        .client()
        .request_capturing_notifications(
            "textDocument/references",
            references_params(&uri, c.line, c.character, Some("refs-token-narrowed")),
            "$/progress",
        )
        .await;

    assert!(resp["error"].is_null(), "references errored: {resp:?}");
    assert!(
        progress.is_empty(),
        "a narrowed (private-method) scope has nothing to prioritize and must \
         send no $/progress notifications, got {progress:?}"
    );

    expect_test::expect![[r#"
        src/Owner.php:2:21-2:28
        src/Owner.php:5:15-5:22"#]]
    .assert_eq(&render_locations(&resp, &s.uri("")));
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

const PRIORITY_AND_REMAINDER_FIXTURE: &str = r#"//- /src/Owner.php
<?php
class Owner {
    public function pro$0cess(): void {}
}

//- /src/PriorityCaller.php
<?php
$o = new Owner();
$o->process();

//- /src/Base.php
<?php
class Base {
    protected Owner $svc;
}

//- /src/Caller.php
<?php
class Caller extends Base {
    public function run(): void {
        $this->svc->process();
    }
}
"#;

/// Public instance-method streaming should prioritize the owner-mentioning
/// subset without re-running the authoritative query over those same files.
/// This fixture has one real call site in the priority batch
/// (`PriorityCaller.php`, which names `Owner`) and one real call site only in
/// the remainder (`Caller.php`, whose type comes from `Base` and never spells
/// `Owner` itself). The progress batch must surface only the former, while the
/// final response still contains both.
#[tokio::test]
async fn references_with_partial_result_token_streams_priority_subset_and_final_includes_remainder()
{
    let mut s = TestServer::new().await;
    let opened = s.open_fixture(PRIORITY_AND_REMAINDER_FIXTURE).await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);

    let (resp, progress) = s
        .client()
        .request_capturing_notifications(
            "textDocument/references",
            references_params(&uri, c.line, c.character, Some("refs-token-split")),
            "$/progress",
        )
        .await;

    assert!(resp["error"].is_null(), "references errored: {resp:?}");
    assert!(
        !progress.is_empty(),
        "expected at least one $/progress notification, got none"
    );

    let streamed = progress
        .iter()
        .map(|notif| {
            let batch = json!({ "result": notif["params"]["value"] });
            render_locations(&batch, &s.uri(""))
        })
        .collect::<Vec<_>>()
        .join("\n");
    expect_test::expect![[r#"
        src/Owner.php:2:20-2:27
        src/PriorityCaller.php:2:4-2:11"#]]
    .assert_eq(&streamed);

    expect_test::expect![[r#"
        src/Caller.php:3:20-3:27
        src/Owner.php:2:20-2:27
        src/PriorityCaller.php:2:4-2:11"#]]
    .assert_eq(&render_locations(&resp, &s.uri("")));
}

const CASE_DIVERGENT_FIXTURE: &str = r#"//- /src/Owner.php
<?php
class Owner {
    public function pro$0cess(): void {}
}

//- /src/CaseDivergent.php
<?php
class CaseDivergent {
    public function run(): void {
        $x = new OWNER();
        $x->process();
    }
}
"#;

/// The priority-streaming partition's owner-mention check must be
/// ASCII-case-insensitive like PHP's own class resolution (and like mir's
/// own candidate gate) — a file that only spells the owner class in a
/// different case (`new OWNER()` for `class Owner`) is still a genuine
/// reference and must stream in the priority batch, not wait for the
/// authoritative pass.
#[serial_test::serial]
#[tokio::test]
async fn references_priority_batch_matches_owner_mention_case_insensitively() {
    let mut s = TestServer::new().await;
    let opened = s.open_fixture(CASE_DIVERGENT_FIXTURE).await;
    let c = opened.cursor().clone();
    let uri = s.uri(&c.path);

    let (resp, progress) = s
        .client()
        .request_capturing_notifications(
            "textDocument/references",
            references_params(&uri, c.line, c.character, Some("refs-token-case")),
            "$/progress",
        )
        .await;

    assert!(resp["error"].is_null(), "references errored: {resp:?}");

    let final_locations = render_locations(&resp, &s.uri(""));
    assert!(
        final_locations.contains("CaseDivergent.php"),
        "the case-divergent call site must appear in the final response:\n{final_locations}"
    );

    let streamed = progress
        .iter()
        .map(|notif| {
            let batch = json!({ "result": notif["params"]["value"] });
            render_locations(&batch, &s.uri(""))
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        streamed.contains("CaseDivergent.php"),
        "case-divergent owner mention (`new OWNER()` for `class Owner`) must \
         stream in the priority batch, not only the authoritative response:\n{streamed}"
    );
}
