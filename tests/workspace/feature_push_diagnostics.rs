//! textDocument/publishDiagnostics (push model) tests.
//!
//! Editors that do not issue pull requests (older Neovim, some Helix configs)
//! rely entirely on the server sending notifications when documents change.
//! These tests verify that `didChange` events produce correct push notifications
//! for single-file scenarios. Cross-file republish is covered separately in
//! `feature_incremental.rs`.

use super::*;
use expect_test::expect;

// ── did_change: single-file push ─────────────────────────────────────────────

/// Introducing a parse error via didChange sends a push with that error.
#[tokio::test]
async fn did_change_pushes_on_parse_error_introduction() {
    let mut s = TestServer::new().await;
    s.open("a.php", "<?php\nfunction ok(): void {}\n").await;

    let notif = s.change("a.php", 2, "<?php\nclass {\n").await;
    expect![[r#"
        1:6-1:7 [1] SyntaxError: expected class name, found '{'
        2:0-2:0 [1] SyntaxError: expected '}', found end of file"#]]
    .assert_eq(&render_diagnostics_notification(&notif));
}

/// Fixing a parse error via didChange sends an empty push.
#[tokio::test]
async fn did_change_pushes_empty_after_parse_error_fixed() {
    let mut s = TestServer::new().await;
    s.open("a.php", "<?php\nclass {\n").await;

    let notif = s.change("a.php", 2, "<?php\nclass Foo {}\n").await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif));
}

/// Introducing a semantic error via didChange sends a push with that error.
#[tokio::test]
async fn did_change_pushes_on_semantic_error_introduction() {
    let mut s = TestServer::new().await;
    s.open("a.php", "<?php\n").await;

    let notif = s.change("a.php", 2, "<?php\nundefined_fn();\n").await;
    expect!["1:0-1:14 [1] UndefinedFunction: Function undefined_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

/// Fixing a semantic error via didChange sends an empty push.
#[tokio::test]
async fn did_change_pushes_empty_after_semantic_error_fixed() {
    let mut s = TestServer::new().await;
    s.open("a.php", "<?php\nundefined_fn();\n").await;

    let notif = s
        .change("a.php", 2, "<?php\nfunction defined_fn(): void {}\n")
        .await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif));
}

/// publishDiagnostics always replaces the complete prior set. After changing
/// from a parse error to a semantic error, only the semantic error must appear;
/// the old parse error must not linger.
#[tokio::test]
async fn did_change_push_replaces_entire_prior_set() {
    let mut s = TestServer::new().await;
    // Open with a parse error.
    s.open("a.php", "<?php\nclass {\n").await;

    // Change to semantic-only error — the parse error must vanish entirely.
    let notif = s.change("a.php", 2, "<?php\nmissing_fn();\n").await;
    expect!["1:0-1:12 [1] UndefinedFunction: Function missing_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

/// Changing from a semantic error to a parse error replaces the set in the
/// other direction — only parse errors survive.
#[tokio::test]
async fn did_change_push_replaces_semantic_with_parse_errors() {
    let mut s = TestServer::new().await;
    // Open with a semantic error.
    s.open("a.php", "<?php\ngone_fn();\n").await;

    // Change to parse error — the semantic error must vanish entirely.
    let notif = s.change("a.php", 2, "<?php\nclass {\n").await;
    expect![[r#"
        1:6-1:7 [1] SyntaxError: expected class name, found '{'
        2:0-2:0 [1] SyntaxError: expected '}', found end of file"#]]
    .assert_eq(&render_diagnostics_notification(&notif));
}

/// Multiple sequential changes converge to the final state's diagnostics.
#[tokio::test]
async fn did_change_push_reflects_final_change_state() {
    let mut s = TestServer::new().await;
    s.open("a.php", "<?php\n").await;

    // First change introduces an error.
    s.change("a.php", 2, "<?php\nbad_fn();\n").await;
    // Second change fixes it.
    let notif = s
        .change("a.php", 3, "<?php\nfunction good(): void {}\n")
        .await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif));
}
