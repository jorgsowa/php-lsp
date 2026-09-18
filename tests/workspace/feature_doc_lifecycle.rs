//! Document lifecycle: didClose, didSave, willSave, willSaveWaitUntil,
//! didChange, and basic endpoint wiring (documentLink, inlineValue).

use super::*;

use expect_test::expect;
use serde_json::Value;

use crate::common::render_text_edits;

// --- did_close ---

#[tokio::test]
async fn did_close_clears_diagnostics() {
    let mut server = TestServer::new().await;
    let uri = server.uri("close_test.php");

    let open_notif = server.open("close_test.php", "<?php function() {}\n").await;
    let open_rendered = render_diagnostics_notification(&open_notif);
    expect![[r#"
        0:6-0:19 [3] MissingClosureReturnType: Closure has no return type annotation
        1:0-1:1 [1] SyntaxError: expected ';' after expression"#]]
    .assert_eq(&open_rendered);

    server.close("close_test.php").await;
    let close_notif = server.client().wait_for_diagnostics(&uri).await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&close_notif));
}

/// Regression: closing a file with unsaved edits must not leave the discarded
/// buffer in the cross-file index. did_close used to drop the editor buffer but
/// never re-read disk, so workspace symbols / references kept resolving against
/// the discarded edit. The fix re-syncs from disk on close.
#[tokio::test]
async fn did_close_resyncs_index_from_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let disk = "<?php\nclass DiskWidget {}\n";
    std::fs::write(tmp.path().join("widget.php"), disk).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("widget.php", disk).await;

    // Edit the buffer without saving: rename the class. While open, the buffer
    // is authoritative, so the index reflects the edited name.
    s.change("widget.php", 2, "<?php\nclass BufferWidget {}\n")
        .await;
    expect!["Class       BufferWidget @ widget.php:1"]
        .assert_eq(&s.snapshot_workspace_symbols("BufferWidget").await);

    // Close without saving — the edit is discarded. Cross-file lookups must now
    // resolve against disk (DiskWidget), not the discarded buffer (BufferWidget).
    let uri = s.uri("widget.php");
    s.close("widget.php").await;
    // did_close re-reads disk then publishes empty diagnostics last; waiting for
    // that publish guarantees the disk re-sync has landed before we query.
    s.client().wait_for_diagnostics(&uri).await;
    expect!["<no symbols>"].assert_eq(&s.snapshot_workspace_symbols("BufferWidget").await);
    expect!["Class       DiskWidget @ widget.php:1"]
        .assert_eq(&s.snapshot_workspace_symbols("DiskWidget").await);
}

#[tokio::test]
async fn did_close_unopened_does_not_crash() {
    let mut server = TestServer::new().await;
    let uri = server.uri("never_opened.php");

    server.close("never_opened.php").await;
    let notif = server.client().wait_for_diagnostics(&uri).await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif));
}

// --- did_save ---

#[tokio::test]
async fn did_save_republishes_empty_diagnostics_for_clean_file() {
    let mut server = TestServer::new().await;
    server.open("save_clean.php", "<?php\n").await;

    let save_notif = server.save("save_clean.php").await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&save_notif));
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
    expect!["2:0-2:20 [1] DuplicateFunction: Function doWork() has already been defined"]
        .assert_eq(&render_diagnostics_notification(&open_notif));

    let save_notif = server.save("save_dup.php").await;
    expect!["2:0-2:20 [1] DuplicateFunction: Function doWork() has already been defined"]
        .assert_eq(&render_diagnostics_notification(&save_notif));
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
    expect!["2:4-2:20 [1] UndefinedFunction: Function nonexistent_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&open_notif));

    let save_notif = server.save("save_semantic.php").await;
    expect!["2:4-2:20 [1] UndefinedFunction: Function nonexistent_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&save_notif));
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
//
// `willSaveWaitUntil` is a request that returns formatting edits to be applied
// before save. If no formatter is available, it returns null. Otherwise it returns
// a TextEdit array with the formatting changes.

#[tokio::test]
async fn will_save_wait_until_returns_null_or_empty_for_formatted_file() {
    let mut server = TestServer::new().await;
    server.open("wswu_clean.php", "<?php\n").await;

    let resp = server.will_save_wait_until("wswu_clean.php").await;
    assert!(resp["error"].is_null(), "unexpected error: {resp:?}");

    expect![r#"(no formatter available)"#].assert_eq(&render_text_edits(&resp));
}

/// willSaveWaitUntil delegates to the same php-cs-fixer/phpcbf formatter as
/// textDocument/formatting (see `src/editing/formatting.rs`), and CI's
/// setup-php step uses `tools: none`, so neither is ever installed — this
/// always takes the "no formatter" path in every environment this suite runs
/// in (see `formatting_returns_null_without_external_formatter` in
/// feature_formatting.rs).
#[tokio::test]
async fn will_save_wait_until_returns_null_without_external_formatter() {
    let mut server = TestServer::new().await;
    server
        .open(
            "wswu_formatted.php",
            "<?php\n\nfunction greet(): void\n{\n}\n",
        )
        .await;

    let resp = server.will_save_wait_until("wswu_formatted.php").await;
    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

#[tokio::test]
async fn will_save_wait_until_returns_null_for_unformatted_file_without_external_formatter() {
    let mut server = TestServer::new().await;
    server
        .open("wswu_ugly.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.will_save_wait_until("wswu_ugly.php").await;
    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

#[tokio::test]
async fn will_save_wait_until_on_unopened_file_returns_null() {
    // If the file is not open in the editor, willSaveWaitUntil should still
    // handle it gracefully (even though LSP spec says it's for open documents).
    let mut server = TestServer::new().await;

    let resp = server.will_save_wait_until("wswu_never_opened.php").await;
    assert!(resp["error"].is_null(), "unexpected error: {resp:?}");

    // Result should be null because the file is not in the document store
    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

#[tokio::test]
async fn will_save_wait_until_on_empty_file() {
    let mut server = TestServer::new().await;
    server.open("wswu_empty.php", "").await;

    let resp = server.will_save_wait_until("wswu_empty.php").await;
    assert!(resp["error"].is_null(), "unexpected error: {resp:?}");

    // Empty file should return null or no edits needed
    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
}

#[tokio::test]
async fn will_save_wait_until_returns_null_without_php_tag_and_without_external_formatter() {
    // PHP snippets without <?php tag should be handled gracefully; no
    // formatter is installed in this suite's environment (see
    // will_save_wait_until_returns_null_without_external_formatter above),
    // so this deterministically returns null.
    let mut server = TestServer::new().await;
    server.open("wswu_no_tag.php", "function test( ){}\n").await;

    let resp = server.will_save_wait_until("wswu_no_tag.php").await;
    expect!["(no formatter available)"].assert_eq(&render_text_edits(&resp));
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

    expect![[r#"
        ```php
        function updated()
        ```"#]]
    .assert_eq(&render_hover(&resp));
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
    let out = if links.is_empty() {
        "<empty>".to_owned()
    } else {
        links
            .iter()
            .map(|l| {
                let start = &l["range"]["start"];
                let line = start["line"].as_u64().unwrap_or(0);
                let col = start["character"].as_u64().unwrap_or(0);
                let target = l["target"].as_str().unwrap_or("?");
                // Replace absolute file path with a stable placeholder.
                let display = if let Some(rest) = target.rfind('/').map(|i| &target[i + 1..]) {
                    rest.to_owned()
                } else {
                    target.to_owned()
                };
                format!("{line}:{col} -> {display}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    expect!["1:14 -> autoload.php"].assert_eq(&out);
}

// --- cross-file analysis freshness ---
//
// When a dependency's method return type changes, the dependent's cross-file
// variable type must refresh even if the dependent file itself is untouched.

/// Regression guard for cross-file type freshness after a dependency edit.
#[tokio::test]
async fn dependency_edit_refreshes_cross_file_hover_type() {
    let mut server = TestServer::new().await;
    server
        .open(
            "maker.php",
            "<?php\nclass Maker { public function make(): Apple { return new Apple(); } }\nclass Apple {}\nclass Banana {}\n",
        )
        .await;
    server
        .open(
            "use_maker.php",
            "<?php\n$m = new Maker();\n$x = $m->make();\necho $x;\n",
        )
        .await;

    // Hover `$x` (the use on line 3) — its type comes from Maker::make() in the
    // other file.
    let before = server.hover("use_maker.php", 3, 5).await;
    expect!["`$x` `Apple`"].assert_eq(&render_hover(&before));

    // Change ONLY maker.php so make() returns Banana; use_maker.php is untouched.
    server
        .change(
            "maker.php",
            2,
            "<?php\nclass Maker { public function make(): Banana { return new Banana(); } }\nclass Apple {}\nclass Banana {}\n",
        )
        .await;

    let after = server.hover("use_maker.php", 3, 5).await;
    expect!["`$x` `Banana`"].assert_eq(&render_hover(&after));
}

/// Same cross-file freshness guard as the hover test, but for the *completion*
/// surface (a separate code path that also reads `cached_analysis`).
#[tokio::test]
async fn dependency_edit_refreshes_cross_file_completion_members() {
    let mut server = TestServer::new().await;
    server
        .open(
            "dep.php",
            "<?php\nclass Maker { public function make(): Alpha { return new Alpha(); } }\nclass Alpha { public function alpha(): void {} }\nclass Beta { public function beta(): void {} }\n",
        )
        .await;
    server
        .open(
            "uses.php",
            "<?php\n$m = new Maker();\n$x = $m->make();\n$x->\n",
        )
        .await;

    let before = server.completion("uses.php", 3, 4).await;
    expect!["Method      alpha"].assert_eq(&render_completion(&before));

    // make() now returns Beta; uses.php is untouched.
    server
        .change(
            "dep.php",
            2,
            "<?php\nclass Maker { public function make(): Beta { return new Beta(); } }\nclass Alpha { public function alpha(): void {} }\nclass Beta { public function beta(): void {} }\n",
        )
        .await;

    let after = server.completion("uses.php", 3, 4).await;
    expect!["Method      beta"].assert_eq(&render_completion(&after));
}
