//! Document lifecycle: didClose, didSave, willSave, willSaveWaitUntil,
//! didChange, and basic endpoint wiring (documentLink, inlineValue).

mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::Value;

/// Render a `publishDiagnostics` notification (or a `didSave` /
/// `didChange` reply that has the same shape) as one line per diagnostic:
/// `L:C-L:C [severity] code: message`. Severity is the LSP enum
/// (1=Error, 2=Warning, 3=Info, 4=Hint). Sorted for determinism.
fn render_diagnostics_notification(notif: &Value) -> String {
    let diags = notif["params"]["diagnostics"].as_array();
    let Some(diags) = diags else {
        return "<no diagnostics field>".to_owned();
    };
    if diags.is_empty() {
        return "<empty>".to_owned();
    }
    let mut rows: Vec<String> = diags
        .iter()
        .map(|d| {
            let r = &d["range"];
            let sev = d["severity"].as_u64().unwrap_or(0);
            let code = d["code"].as_str().unwrap_or("?");
            let msg = d["message"].as_str().unwrap_or("");
            format!(
                "{}:{}-{}:{} [{sev}] {code}: {msg}",
                r["start"]["line"].as_u64().unwrap_or(0),
                r["start"]["character"].as_u64().unwrap_or(0),
                r["end"]["line"].as_u64().unwrap_or(0),
                r["end"]["character"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

// --- did_close ---

#[tokio::test]
async fn did_close_clears_diagnostics() {
    let mut server = TestServer::new().await;
    let uri = server.uri("close_test.php");

    let open_notif = server.open("close_test.php", "<?php function() {}\n").await;
    assert!(
        !open_notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected parse errors before close: {open_notif:?}"
    );

    server.close("close_test.php").await;
    let close_notif = server.client().wait_for_diagnostics(&uri).await;
    assert!(
        close_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "expected empty diagnostics after close: {close_notif:?}"
    );
}

#[tokio::test]
async fn did_close_unopened_does_not_crash() {
    let mut server = TestServer::new().await;
    let uri = server.uri("never_opened.php");

    server.close("never_opened.php").await;
    let notif = server.client().wait_for_diagnostics(&uri).await;
    assert!(
        notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "expected empty diagnostics for never-opened file: {notif:?}"
    );
}

// --- did_save ---

#[tokio::test]
async fn did_save_republishes_empty_diagnostics_for_clean_file() {
    let mut server = TestServer::new().await;
    server.open("save_clean.php", "<?php\n").await;

    let save_notif = server.save("save_clean.php").await;
    assert!(
        save_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "expected no diagnostics after save of clean file: {save_notif:?}"
    );
}

#[tokio::test]
async fn did_save_republishes_diagnostics_for_duplicate_functions() {
    let mut server = TestServer::new().await;
    let open_notif = server
        .open(
            "save_dup.php",
            "<?php\nfunction doWork() {}\nfunction doWork() {}\n",
        )
        .await;
    assert!(
        !open_notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected duplicate-declaration diagnostic on open: {open_notif:?}"
    );

    let save_notif = server.save("save_dup.php").await;
    assert!(
        save_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .len()
            >= 1,
        "expected >=1 diagnostic after save with duplicate functions: {save_notif:?}"
    );
}

#[tokio::test]
async fn did_save_republishes_semantic_diagnostics() {
    // Regression: did_save was manually building parse+dup-decl diagnostics
    // and omitting the semantic pass. publishDiagnostics *replaces* the prior
    // set, so saving a file with semantic errors would silently clear them.
    let mut server = TestServer::new().await;
    let open_notif = server
        .open(
            "save_semantic.php",
            "<?php\nfunction _wrap(): void {\n    nonexistent_fn();\n}\n",
        )
        .await;
    assert!(
        !open_notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected semantic diagnostic on open: {open_notif:?}"
    );

    let save_notif = server.save("save_semantic.php").await;
    assert!(
        !save_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "did_save must republish semantic diagnostics, got empty list: {save_notif:?}"
    );
}

// --- willSave ---
//
// `willSave` is a void notification — the spec lets the server do nothing,
// and that's exactly what this server does (formatting on save is wired
// through `willSaveWaitUntil` instead). The tests below pin that behaviour:
// the handler must never crash, never mutate the buffer, never publish
// diagnostics, and never disturb adjacent lifecycle handlers.

#[tokio::test]
async fn will_save_keeps_document_state_unchanged() {
    // Open a file with a known semantic diagnostic, fire `willSave` for all
    // three `TextDocumentSaveReason` values (1=Manual, 2=AfterDelay,
    // 3=FocusOut), then trigger `didSave` and snapshot the diagnostics.
    // If `willSave` mutated the buffer or invalidated cached analysis the
    // post-save diagnostics would shift; identical-to-on-open proves they
    // didn't.
    let mut server = TestServer::new().await;
    let open_notif = server
        .open(
            "ws_state.php",
            "<?php\nfunction _wrap(): void {\n    nonexistent_fn();\n}\n",
        )
        .await;

    expect!["2:4-2:20 [1] UndefinedFunction: Function nonexistent_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&open_notif));

    for reason in [1u32, 2, 3] {
        server.will_save("ws_state.php", reason).await;
    }

    let save_notif = server.save("ws_state.php").await;
    expect!["2:4-2:20 [1] UndefinedFunction: Function nonexistent_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&save_notif));
}

#[tokio::test]
async fn will_save_does_not_publish_diagnostics() {
    // willSave must not trigger a publishDiagnostics — that's didSave's job.
    // If it did, editors that send willSave on every focus-out would see
    // diagnostic flicker.
    let mut server = TestServer::new().await;
    server
        .open("ws_nodiag.php", "<?php\nfunction foo() {}\n")
        .await;

    for reason in [1u32, 2, 3] {
        server.will_save("ws_nodiag.php", reason).await;
    }

    // Round-trip a request to ensure any notification willSave *might* have
    // produced has had a chance to traverse the channel before we drain.
    let hover = server.hover("ws_nodiag.php", 1, 10).await;
    assert!(hover["error"].is_null(), "hover errored: {hover:?}");

    let uris = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(100))
        .await;
    expect!["[]"].assert_eq(&format!("{uris:?}"));
}

#[tokio::test]
async fn will_save_for_unopened_file_does_not_crash() {
    // The LSP spec only requires clients to send willSave for open documents,
    // but a misbehaving client (or a race against didClose) could send it
    // for an unknown URI. The handler must be tolerant — we verify by
    // confirming the server still produces correct diagnostics afterwards.
    let mut server = TestServer::new().await;

    server.will_save("ws_never_opened.php", 1).await;
    server.will_save("ws_never_opened.php", 2).await;
    server.will_save("ws_never_opened.php", 3).await;

    let open_notif = server
        .open(
            "ws_after.php",
            "<?php\nfunction _wrap(): void {\n    nonexistent_fn();\n}\n",
        )
        .await;
    expect!["2:4-2:20 [1] UndefinedFunction: Function nonexistent_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&open_notif));
}

#[tokio::test]
async fn will_save_after_did_close_does_not_crash() {
    // Race: editor closes the file, then a queued willSave from the previous
    // save attempt arrives. The handler must not panic.
    let mut server = TestServer::new().await;
    server
        .open("ws_closed.php", "<?php\nfunction foo() {}\n")
        .await;
    server.close("ws_closed.php").await;
    let _ = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(50))
        .await;

    server.will_save("ws_closed.php", 1).await;

    // Sanity: server still serves new opens correctly.
    let open_notif = server.open("ws_after_close.php", "<?php\n").await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&open_notif));
}

#[tokio::test]
async fn will_save_does_not_disturb_pending_did_change() {
    // willSave between didChange and the resulting diagnostic publish must
    // not cancel or alter the pending parse — the editor relies on the
    // diagnostic for the latest version landing.
    let mut server = TestServer::new().await;
    server.open("ws_change.php", "<?php\n").await;

    // didChange schedules a debounced re-parse; willSave fires while it's
    // in-flight.
    server
        .change(
            "ws_change.php",
            2,
            "<?php\nfunction _wrap(): void {\n    nonexistent_fn();\n}\n",
        )
        .await;
    server.will_save("ws_change.php", 1).await;

    let save_notif = server.save("ws_change.php").await;
    expect!["2:4-2:20 [1] UndefinedFunction: Function nonexistent_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&save_notif));
}

// --- willSaveWaitUntil ---

#[tokio::test]
async fn will_save_wait_until_returns_null_or_empty_for_formatted_file() {
    let mut server = TestServer::new().await;
    server.open("wswu_clean.php", "<?php\n").await;

    let resp = server.will_save_wait_until("wswu_clean.php").await;
    assert!(resp["error"].is_null(), "unexpected error: {resp:?}");
    let result = &resp["result"];
    assert!(
        result.is_null() || result.as_array().map(|a| a.is_empty()).unwrap_or(false),
        "expected null or empty edits for already-formatted file: {resp:?}"
    );
}

#[tokio::test]
async fn will_save_wait_until_returns_null_or_edits_for_unformatted_file() {
    let mut server = TestServer::new().await;
    server
        .open("wswu_ugly.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.will_save_wait_until("wswu_ugly.php").await;
    assert!(resp["error"].is_null(), "unexpected error: {resp:?}");

    let result = &resp["result"];
    if let Some(edits) = result.as_array() {
        for edit in edits {
            assert!(
                edit["range"]["start"].is_object() && edit["range"]["end"].is_object(),
                "edit missing range: {edit:?}"
            );
            assert!(
                edit["newText"].is_string(),
                "edit missing newText: {edit:?}"
            );
        }
    } else {
        assert!(result.is_null(), "expected null or array, got: {result:?}");
    }
}

// --- didChange ---

#[tokio::test]
async fn did_change_updates_document() {
    let mut server = TestServer::new().await;
    server.open("change.php", "<?php\n").await;

    server
        .change("change.php", 2, "<?php\nfunction updated() {}\n")
        .await;

    let resp = server.hover("change.php", 1, 10).await;

    assert!(
        resp["error"].is_null(),
        "hover after change should not error"
    );
}

// --- endpoint wiring ---

#[tokio::test]
async fn document_link_returns_array() {
    let mut server = TestServer::new().await;
    server
        .open("dlink.php", "<?php\nrequire_once 'vendor/autoload.php';\n")
        .await;

    let resp = server.document_link("dlink.php").await;

    assert!(resp["error"].is_null(), "documentLink error: {:?}", resp);
    let links = resp["result"]
        .as_array()
        .expect("documentLink must return an array");
    assert!(
        !links.is_empty(),
        "expected at least one link for require_once path"
    );
}
