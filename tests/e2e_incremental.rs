//! E2E tests for incremental `didChange` correctness.
//!
//! Every real editor session is a long sequence of edits. These tests exercise
//! the edit loop: open a file, send `didChange`, then verify that subsequent
//! hover / definition / references / diagnostics requests see the *new* state,
//! not stale cached results. Directly stresses the salsa-backed parse + index
//! caches introduced on the refactor/salsa-incremental branch.

mod common;

use common::TestServer;

fn has_code(notif: &serde_json::Value, code: &str) -> bool {
    notif["params"]["diagnostics"]
        .as_array()
        .map(|arr| arr.iter().any(|d| d["code"].as_str() == Some(code)))
        .unwrap_or(false)
}

/// Edit a file to introduce a new function, then hover it. If the parse cache
/// wasn't invalidated on `didChange`, hover would return null.
#[tokio::test]
async fn hover_reflects_didchange_new_symbol() {
    let mut server = TestServer::new().await;
    server.open("edit.php", "<?php\n").await;

    server
        .change(
            "edit.php",
            2,
            "<?php\nfunction greeter(string $name): string { return $name; }\n",
        )
        .await;

    let resp = server.hover("edit.php", 1, 10).await;
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("greeter") && contents.contains("string"),
        "hover after didChange must see the new function signature, got: {contents}"
    );
}

/// Cache-invalidation check: populate the definition cache by querying on
/// the V1 name, then rewrite via didChange and query again. A server that
/// failed to invalidate would return the stale V1 result on the V2 query.
#[tokio::test]
async fn definition_cache_is_invalidated_after_didchange() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ren.php",
            "<?php\nfunction oldName(): void {}\noldName();\n",
        )
        .await;

    // Warm the cache on V1. `oldName()` on line 2 must resolve to line 1.
    let resp = server.definition("ren.php", 2, 1).await;
    let loc_v1 = if resp["result"].is_array() {
        resp["result"][0].clone()
    } else {
        resp["result"].clone()
    };
    assert_eq!(
        loc_v1["range"]["start"]["line"].as_u64().unwrap(),
        1,
        "V1 cache warmup failed"
    );

    // V2: different function name at a different declaration column.
    server
        .change(
            "ren.php",
            2,
            "<?php\n\nfunction newName(): void {}\nnewName();\n",
        )
        .await;

    // Query at V2's call site position (line 3, col 1).
    let resp = server.definition("ren.php", 3, 1).await;
    let result = &resp["result"];
    assert!(!result.is_null(), "newName() must resolve after didChange");
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    // V2 declaration is on line 2 (was line 1 in V1). A stale cache would
    // return line 1 or the old range.
    assert_eq!(
        loc["range"]["start"]["line"].as_u64().unwrap(),
        2,
        "expected V2 line (2), stale V1 result would be line 1"
    );
}

/// References must reflect the post-edit state — an added usage shows up,
/// and a removed one disappears.
#[tokio::test]
async fn references_reflect_didchange_additions_and_removals() {
    let mut server = TestServer::new().await;
    server
        .open("refs.php", "<?php\nfunction target(): void {}\ntarget();\n")
        .await;

    // Add a second call site.
    server
        .change(
            "refs.php",
            2,
            "<?php\nfunction target(): void {}\ntarget();\ntarget();\n",
        )
        .await;

    let resp = server.references("refs.php", 1, 9, false).await;
    let refs = resp["result"].as_array().expect("references array");
    assert_eq!(
        refs.len(),
        2,
        "expected both call sites after edit: {refs:?}"
    );

    // Now remove one call site.
    server
        .change(
            "refs.php",
            3,
            "<?php\nfunction target(): void {}\ntarget();\n",
        )
        .await;

    let resp = server.references("refs.php", 1, 9, false).await;
    let refs = resp["result"].as_array().expect("references array");
    assert_eq!(
        refs.len(),
        1,
        "expected 1 call site after removal: {refs:?}"
    );
}

/// Diagnostics must update after fixing a parse error without leaving stale
/// errors behind.
#[tokio::test]
async fn diagnostics_replaced_not_appended_on_didchange() {
    let mut server = TestServer::new().await;
    let notif = server.open("d.php", "<?php\nbroken(;\n").await;
    let first_count = notif["params"]["diagnostics"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(first_count > 0, "expected parse error on open");

    let notif = server.change("d.php", 2, "<?php\n").await;
    let diags = notif["params"]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        diags.is_empty(),
        "diagnostics from prior version must be cleared, got: {diags:?}"
    );
}

/// Cross-file *re-analysis on demand*: after the dependency changes, a new
/// didChange on the dependent yields fresh diagnostics. This documents the
/// current behavior — re-analysis is triggered by the next edit, not pushed
/// proactively (see `cross_file_diagnostics_republish_on_dependency_change`
/// below for the ideal behavior, tracked as ignored).
#[tokio::test]
async fn cross_file_diagnostics_refresh_on_next_didchange() {
    let mut server = TestServer::new().await;
    server.open("dep.php", "<?php\nclass Widget {}\n").await;
    let notif = server.open("user.php", "<?php\n$w = new Widget();\n").await;
    assert!(
        !has_code(&notif, "UndefinedClass"),
        "Widget is defined — expected no UndefinedClass initially: {:?}",
        notif["params"]["diagnostics"]
    );

    // Rename the class in dep.php so Widget no longer exists anywhere.
    server
        .change("dep.php", 2, "<?php\nclass Gadget {}\n")
        .await;

    // Re-trigger analysis on user.php by sending an identical didChange.
    let notif = server
        .change("user.php", 2, "<?php\n$w = new Widget();\n")
        .await;
    assert!(
        has_code(&notif, "UndefinedClass"),
        "after renaming Widget→Gadget in dep.php, user.php must report UndefinedClass: {:?}",
        notif["params"]["diagnostics"]
    );
}

/// When a dependency changes, the server proactively republishes
/// diagnostics for every open dependent file — no extra didChange required.
#[tokio::test]
async fn cross_file_diagnostics_republish_on_dependency_change() {
    let mut server = TestServer::new().await;
    server.open("dep2.php", "<?php\nclass Widget2 {}\n").await;
    server
        .open("user2.php", "<?php\n$w = new Widget2();\n")
        .await;

    server
        .change("dep2.php", 2, "<?php\nclass Gadget2 {}\n")
        .await;

    // Expect a publishDiagnostics notification for user2.php without any
    // further didChange.
    let uri = server.uri("user2.php");
    let notif = server.client().wait_for_diagnostics(&uri).await;
    assert!(
        has_code(&notif, "UndefinedClass"),
        "expected proactive UndefinedClass on user2.php after dependency edit"
    );
}

/// Fire five didChange notifications back-to-back **without** awaiting
/// publishDiagnostics between them, then hover on the final state. This
/// genuinely stresses the 100 ms debounce — intermediate versions are
/// superseded before they parse. A correct server converges on V6.
#[tokio::test]
async fn true_burst_didchange_converges_to_final_text() {
    let mut server = TestServer::new().await;
    server.open("burst.php", "<?php\n").await;

    let uri = server.uri("burst.php");
    // Raw notifies — no wait_for_diagnostics in the loop.
    for v in 2..=6 {
        let text = format!("<?php\nfunction f{v}(): void {{}}\n");
        server
            .client()
            .notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": uri, "version": v },
                    "contentChanges": [{ "text": text }],
                }),
            )
            .await;
    }

    // Drain publishDiagnostics messages until we see one from the final
    // version text. The loop tolerates intermediate notifications the
    // debounce may or may not have produced.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if std::time::Instant::now() >= deadline {
            panic!("timed out waiting for burst to settle");
        }
        // Probe with hover — once it reflects f6 the server has caught up.
        let resp = server.hover("burst.php", 1, 10).await;
        let contents = resp["result"]["contents"].to_string();
        if contents.contains("f6") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Re-opening (didClose + didOpen) must not leave ghost symbols. A second
/// open with identical text should behave the same as the first.
#[tokio::test]
async fn reopen_does_not_duplicate_symbols() {
    let mut server = TestServer::new().await;
    let src = "<?php\nfunction once(): void {}\nonce();\n";
    server.open("reopen.php", src).await;

    let uri = server.uri("reopen.php");
    server
        .client()
        .notify(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;

    server.open("reopen.php", src).await;

    let resp = server.references("reopen.php", 1, 9, true).await;
    let refs = resp["result"].as_array().expect("references array");
    assert_eq!(
        refs.len(),
        2,
        "expected declaration + 1 call, not duplicates after reopen: {refs:?}"
    );
}

// ── cross-file diagnostic republish: edge cases ─────────────────────────────

/// Symmetric to the rename case: when a dependency *appears* (did_open of a
/// file that defines a previously-missing symbol), the dependent's
/// UndefinedClass must clear without re-editing the dependent.
#[tokio::test]
async fn cross_file_diagnostic_clears_when_dependency_opened() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("user_open.php", "<?php\n$w = new ProvidedClass();\n")
        .await;
    assert!(
        has_code(&notif, "UndefinedClass"),
        "expected UndefinedClass before dep is opened: {:?}",
        notif["params"]["diagnostics"]
    );

    // Opening a file that defines ProvidedClass should clear the error in user_open.php.
    server
        .open("provider.php", "<?php\nclass ProvidedClass {}\n")
        .await;

    let user_uri = server.uri("user_open.php");
    let notif = server.client().wait_for_diagnostics(&user_uri).await;
    assert!(
        !has_code(&notif, "UndefinedClass"),
        "expected UndefinedClass cleared after dep opened: {:?}",
        notif["params"]["diagnostics"]
    );
}

/// Two open dependents must both receive proactive republishes after a
/// dependency change. Order of dependent publishes is not guaranteed.
#[tokio::test]
async fn cross_file_republish_fans_out_to_multiple_dependents() {
    let mut server = TestServer::new().await;
    server
        .open("dep_fan.php", "<?php\nclass FanWidget {}\n")
        .await;
    server
        .open("u1_fan.php", "<?php\n$w = new FanWidget();\n")
        .await;
    server
        .open("u2_fan.php", "<?php\n$w = new FanWidget();\n")
        .await;

    // Each open() above triggers cross-file republishes for already-open
    // files; those leftover publishes sit in the queue with stale (pre-
    // rename) content. Drain them so the assertion below only inspects
    // publishes triggered by the change.
    let _ = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(200))
        .await;

    // Trigger: rename FanWidget away. server.change() consumes dep_fan's
    // publish; the dependent publishes for u1/u2 stay queued.
    server
        .change("dep_fan.php", 2, "<?php\nclass FanGadget {}\n")
        .await;

    let u1 = server.uri("u1_fan.php");
    let u2 = server.uri("u2_fan.php");
    let notifs = server
        .client()
        .wait_for_diagnostics_multi(&[&u1, &u2])
        .await;

    for (label, uri) in [("u1", &u1), ("u2", &u2)] {
        let notif = notifs
            .get(uri)
            .unwrap_or_else(|| panic!("missing publish for {label} ({uri})"));
        assert!(
            has_code(notif, "UndefinedClass"),
            "{label}: expected UndefinedClass after FanWidget rename, got: {:?}",
            notif["params"]["diagnostics"]
        );
    }
}

/// A closed file must NOT receive proactive republishes — the server should
/// only push to currently-open documents. Editing a published-only file
/// would violate the LSP open-document contract.
#[tokio::test]
async fn cross_file_republish_skips_closed_files() {
    let mut server = TestServer::new().await;
    server
        .open("dep_closed.php", "<?php\nclass ClosedDep {}\n")
        .await;
    server
        .open("user_closed.php", "<?php\n$w = new ClosedDep();\n")
        .await;

    // Close user_closed.php. did_close itself sends one final empty publish
    // to clear the editor — drain any pending publish for that URI here.
    let user_uri = server.uri("user_closed.php");
    server.close("user_closed.php").await;
    let _ = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(200))
        .await;

    // Now mutate dep so it WOULD have caused a republish if user_closed.php
    // were still open.
    server
        .change("dep_closed.php", 2, "<?php\nclass ClosedDepRenamed {}\n")
        .await;

    // No publish should arrive for the closed file. Wait briefly to confirm.
    let seen = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(300))
        .await;
    assert!(
        !seen.iter().any(|u| u == &user_uri),
        "closed file received an unexpected publishDiagnostics: {seen:?}"
    );
}

/// Editing a file with no dependents still sends republishes to other open
/// files (for simplicity — Salsa caches make this cheap), and those
/// republishes must use `[]` not `null` for the diagnostics field, since
/// `null` would violate the LSP spec.
#[tokio::test]
async fn cross_file_republish_uses_empty_array_for_clean_dependent() {
    let mut server = TestServer::new().await;
    // Two independent files, both clean. They don't reference each other.
    server
        .open("clean_a.php", "<?php\nfunction aa(): void {}\n")
        .await;
    server
        .open("clean_b.php", "<?php\nfunction bb(): void {}\n")
        .await;

    // Edit clean_a; clean_b is unaffected but receives a cache-hit republish.
    server
        .change("clean_a.php", 2, "<?php\nfunction aaaa(): void {}\n")
        .await;

    let b_uri = server.uri("clean_b.php");
    let notif = server.client().wait_for_diagnostics(&b_uri).await;
    let diags = &notif["params"]["diagnostics"];
    assert!(
        diags.is_array(),
        "diagnostics must be an array (LSP requires the field), got: {diags:?}"
    );
    assert!(
        diags.as_array().unwrap().is_empty(),
        "clean_b is independent — expected empty diagnostics, got: {diags:?}"
    );
}

/// A dependent with a parse error must keep its parse diagnostics through
/// a cross-file republish — the republish path uses cached parse diagnostics
/// rather than re-parsing, so the error must not silently drop.
#[tokio::test]
async fn cross_file_republish_preserves_dependent_parse_errors() {
    let mut server = TestServer::new().await;
    let notif = server.open("broken.php", "<?php\nbroken(;\n").await;
    let original_count = notif["params"]["diagnostics"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        original_count > 0,
        "expected parse error in broken.php on open"
    );

    // Open an unrelated file; this triggers a cross-file republish loop that
    // recomputes broken.php's diagnostics. Parse errors must be preserved.
    server
        .open("trigger.php", "<?php\nclass Triggered {}\n")
        .await;

    let broken_uri = server.uri("broken.php");
    let notif = server.client().wait_for_diagnostics(&broken_uri).await;
    let count = notif["params"]["diagnostics"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        count >= original_count,
        "cross-file republish dropped parse diagnostics: had {original_count}, now {count}: {:?}",
        notif["params"]["diagnostics"]
    );
}
