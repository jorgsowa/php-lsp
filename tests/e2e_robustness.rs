//! Negative-path robustness tests. Every LSP request must return a valid
//! JSON-RPC response — a null result is fine, a crash or error is not.
//! These exercise the edges: empty files, syntax errors, cursors past EOF,
//! unknown symbols. The server survives; clients degrade gracefully.

mod common;

use common::TestServer;

/// Hover on an empty file must return null without erroring.
#[tokio::test]
async fn hover_on_empty_file_returns_null_not_error() {
    let mut server = TestServer::new().await;
    server.open("empty.php", "").await;

    let resp = server.hover("empty.php", 0, 0).await;
    assert!(
        resp["error"].is_null(),
        "hover errored on empty file: {resp:?}"
    );
    assert!(
        resp["result"].is_null(),
        "hover on empty file should be null, got: {:?}",
        resp["result"]
    );
}

/// Hover well past EOF must not panic — the server clamps or returns null.
#[tokio::test]
async fn hover_past_eof_does_not_crash() {
    let mut server = TestServer::new().await;
    server
        .open("short.php", "<?php\nfunction f(): void {}\n")
        .await;

    // Line 500, char 500 — way past the end of the two-line file.
    let resp = server.hover("short.php", 500, 500).await;
    assert!(resp["error"].is_null(), "hover past EOF errored: {resp:?}");
    // result may be null or a best-effort response — both are acceptable.
}

/// Goto-definition on an unknown symbol returns null; no error, no crash.
#[tokio::test]
async fn definition_on_unknown_symbol_returns_null() {
    let mut server = TestServer::new().await;
    server
        .open("unk.php", "<?php\n$x = new UnknownClass();\n")
        .await;

    let resp = server.definition("unk.php", 1, 13).await;
    assert!(resp["error"].is_null(), "definition errored: {resp:?}");
    let result = &resp["result"];
    let is_empty = result.is_null() || result.as_array().map(|a| a.is_empty()).unwrap_or(false);
    assert!(
        is_empty,
        "unknown symbol should have no definition, got: {result:?}"
    );
}

/// A file with a severe parse error must still accept feature requests
/// without the server returning an error or hanging.
#[tokio::test]
async fn requests_on_parse_error_file_do_not_error() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("broken.php", "<?php\nfunction f( $x { // missing ): body\n")
        .await;

    // Parse diagnostics must fire (non-empty).
    let diags = notif["params"]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !diags.is_empty(),
        "expected parse diagnostics for broken source"
    );

    // Hover, document symbols, folding — all must respond without error.
    let resp = server.hover("broken.php", 1, 10).await;
    assert!(resp["error"].is_null(), "hover errored: {resp:?}");

    let resp = server.document_symbols("broken.php").await;
    assert!(resp["error"].is_null(), "documentSymbol errored: {resp:?}");

    let resp = server.folding_range("broken.php").await;
    assert!(resp["error"].is_null(), "foldingRange errored: {resp:?}");
}

/// References on an unopened URI returns null/empty — must not error.
#[tokio::test]
async fn references_on_unopened_uri_returns_empty() {
    let mut server = TestServer::new().await;
    let resp = server.references("ghost.php", 0, 0, false).await;
    assert!(resp["error"].is_null(), "references errored: {resp:?}");
    let result = &resp["result"];
    let is_empty = result.is_null() || result.as_array().map(|a| a.is_empty()).unwrap_or(false);
    assert!(
        is_empty,
        "references on unopened file should be empty, got: {result:?}"
    );
}

/// Rename on a symbol with no matches must return a valid (possibly empty)
/// WorkspaceEdit and never an RPC error.
#[tokio::test]
async fn rename_on_nonexistent_symbol_does_not_error() {
    let mut server = TestServer::new().await;
    server.open("rn.php", "<?php\n// nothing to rename\n").await;

    // Target a position inside the comment.
    let resp = server.rename("rn.php", 1, 5, "NewName").await;
    assert!(resp["error"].is_null(), "rename errored: {resp:?}");
    // result may be null or an empty WorkspaceEdit — either is fine.
}
