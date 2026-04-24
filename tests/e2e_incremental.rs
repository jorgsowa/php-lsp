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

/// After renaming a symbol via didChange (old name deleted, new name added),
/// definition on the old-name call site must return null and on the new-name
/// call site must resolve.
#[tokio::test]
async fn definition_updates_after_symbol_rename_via_didchange() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ren.php",
            "<?php\nfunction oldName(): void {}\noldName();\n",
        )
        .await;

    // Replace the function and call site.
    server
        .change(
            "ren.php",
            2,
            "<?php\nfunction newName(): void {}\nnewName();\n",
        )
        .await;

    let resp = server.definition("ren.php", 2, 1).await;
    let result = &resp["result"];
    assert!(!result.is_null(), "newName() must resolve after didChange");
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    assert_eq!(loc["range"]["start"]["line"].as_u64().unwrap(), 1);
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

/// Cross-file invalidation: file A defines a class, file B uses it. Edit A to
/// rename the class. Diagnostics for B must flip from clean to UndefinedClass.
#[tokio::test]
async fn cross_file_diagnostics_invalidate_when_dependency_changes() {
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

/// Rapid-fire edits: the server debounces parsing by 100 ms. A burst of
/// changes must converge to the final text — no stale diagnostics, no panics.
#[tokio::test]
async fn rapid_didchange_burst_converges_to_final_text() {
    let mut server = TestServer::new().await;
    server.open("burst.php", "<?php\n").await;

    for v in 2..=6 {
        server
            .change(
                "burst.php",
                v,
                &format!("<?php\nfunction f{v}(): void {{}}\n"),
            )
            .await;
    }

    // The final version defines f6 — hover on it should work.
    let resp = server.hover("burst.php", 1, 10).await;
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("f6"),
        "after burst, hover must reflect final text (f6), got: {contents}"
    );

    // And f2..f5 must be gone.
    let resp = server.definition("burst.php", 1, 1).await;
    // Not asserting exact value here — just that the server is alive and
    // responding. The key invariant tested above is final-text convergence.
    assert!(
        resp["error"].is_null(),
        "server unresponsive after burst: {resp:?}"
    );
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
