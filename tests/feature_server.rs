//! Server lifecycle and concurrency: initialize, shutdown, protocol stubs,
//! and sustained request interleaving under load.

mod common;

use common::TestServer;

// ── lifecycle ────────────────────────────────────────────────────────────────

// Every (feature-flag key, ServerCapabilities JSON field) pair.
const FEATURE_CAP_PAIRS: &[(&str, &str)] = &[
    ("completion", "completionProvider"),
    ("hover", "hoverProvider"),
    ("definition", "definitionProvider"),
    ("declaration", "declarationProvider"),
    ("references", "referencesProvider"),
    ("documentSymbols", "documentSymbolProvider"),
    ("workspaceSymbols", "workspaceSymbolProvider"),
    ("rename", "renameProvider"),
    ("signatureHelp", "signatureHelpProvider"),
    ("inlayHints", "inlayHintProvider"),
    ("semanticTokens", "semanticTokensProvider"),
    ("selectionRange", "selectionRangeProvider"),
    ("callHierarchy", "callHierarchyProvider"),
    ("documentHighlight", "documentHighlightProvider"),
    ("implementation", "implementationProvider"),
    ("codeAction", "codeActionProvider"),
    ("typeDefinition", "typeDefinitionProvider"),
    ("codeLens", "codeLensProvider"),
    ("formatting", "documentFormattingProvider"),
    ("rangeFormatting", "documentRangeFormattingProvider"),
    ("onTypeFormatting", "documentOnTypeFormattingProvider"),
    ("documentLink", "documentLinkProvider"),
    ("linkedEditingRange", "linkedEditingRangeProvider"),
    ("inlineValues", "inlineValueProvider"),
];

// Capabilities that are always present regardless of feature flags.
const UNCONDITIONAL_CAPS: &[&str] = &[
    "textDocumentSync",
    "foldingRangeProvider",
    "executeCommandProvider",
    "diagnosticProvider",
    "workspace",
    "monikerProvider",
];

#[tokio::test]
async fn all_features_disabled_removes_all_toggleable_capabilities() {
    let mut all_off = serde_json::json!({ "diagnostics": { "enabled": true }, "features": {} });
    for (flag, _) in FEATURE_CAP_PAIRS {
        all_off["features"][flag] = serde_json::json!(false);
    }

    let (_, resp) = TestServer::new_with_options(all_off).await;
    let caps = &resp["result"]["capabilities"];

    for (flag, cap_field) in FEATURE_CAP_PAIRS {
        assert!(
            caps[cap_field].is_null(),
            "expected {cap_field} to be absent when feature '{flag}' is disabled, got: {}",
            caps[cap_field]
        );
    }
    for cap_field in UNCONDITIONAL_CAPS {
        assert!(
            !caps[cap_field].is_null(),
            "expected {cap_field} to remain present (unconditional), got null"
        );
    }
}

#[tokio::test]
async fn all_features_enabled_by_default() {
    let (_, resp) = TestServer::new_with_options(serde_json::json!({
        "diagnostics": { "enabled": true }
    }))
    .await;
    let caps = &resp["result"]["capabilities"];

    for (flag, cap_field) in FEATURE_CAP_PAIRS {
        assert!(
            !caps[cap_field].is_null(),
            "expected {cap_field} to be present by default (feature '{flag}' not mentioned), got null"
        );
    }
}

#[tokio::test]
async fn initialize_returns_server_capabilities() {
    let mut server = TestServer::new().await;
    server
        .open("cap.php", "<?php\nfunction f(): void {}\n")
        .await;
    let resp = server.hover("cap.php", 1, 10).await;
    assert!(
        resp["error"].is_null(),
        "hover should not error if hoverProvider is advertised: {:?}",
        resp
    );
    assert!(
        !resp["result"].is_null(),
        "hover should return a result, confirming textDocumentSync applied the open"
    );
}

#[tokio::test]
async fn shutdown_responds_correctly() {
    let mut server = TestServer::new().await;
    let resp = server.shutdown().await;

    assert!(
        resp["error"].is_null(),
        "shutdown should not error: {:?}",
        resp
    );
    assert!(resp["result"].is_null(), "shutdown result should be null");
}

#[tokio::test]
async fn linked_editing_range_returns_no_error() {
    let mut server = TestServer::new().await;
    server
        .open("linked.php", "<?php\nclass LinkedClass {}\n")
        .await;

    let resp = server.linked_editing_range("linked.php", 1, 6).await;

    assert!(
        resp["error"].is_null(),
        "linkedEditingRange error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected non-null LinkedEditingRanges for class name, got null"
    );
    let ranges = result["ranges"]
        .as_array()
        .expect("expected 'ranges' array in LinkedEditingRanges");
    assert_eq!(
        ranges.len(),
        1,
        "expected exactly one range for LinkedClass"
    );
    assert_eq!(
        ranges[0]["start"],
        serde_json::json!({"line": 1, "character": 6}),
        "range start must point to the L in LinkedClass"
    );
    assert_eq!(
        ranges[0]["end"],
        serde_json::json!({"line": 1, "character": 17}),
        "range end must be after the last char of LinkedClass"
    );
}

// ── concurrency ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn many_files_hover_each_returns_own_signature() {
    let mut server = TestServer::new().await;

    for i in 0..10 {
        let src = format!("<?php\nfunction fn_{i}(int $x): int {{ return $x; }}\n");
        server.open(&format!("c{i}.php"), &src).await;
    }

    for i in 0..10 {
        let resp = server.hover(&format!("c{i}.php"), 1, 10).await;
        let contents = resp["result"]["contents"].to_string();
        assert!(
            contents.contains(&format!("fn_{i}")),
            "file c{i}.php hover must mention fn_{i}, got: {contents}"
        );
    }
}

#[tokio::test]
async fn sustained_hover_volley_all_succeed() {
    let mut server = TestServer::new().await;
    server
        .open(
            "pipe.php",
            "<?php\nfunction pipeHover(int $x): int { return $x; }\n",
        )
        .await;

    for _ in 0..20 {
        let resp = server.hover("pipe.php", 1, 10).await;
        assert!(resp["error"].is_null(), "hover errored in volley: {resp:?}");
        assert!(
            resp["result"]["contents"].to_string().contains("pipeHover"),
            "hover content must stay correct across volley"
        );
    }
}

#[tokio::test]
async fn didchange_followed_by_request_sees_new_state_every_iteration() {
    let mut server = TestServer::new().await;
    server.open("iter.php", "<?php\n").await;

    for v in 2..=8 {
        let src = format!("<?php\nfunction iter_{v}(): int {{ return {v}; }}\niter_{v}();\n");
        server.change("iter.php", v, &src).await;

        let resp = server.hover("iter.php", 1, 10).await;
        let contents = resp["result"]["contents"].to_string();
        assert!(
            contents.contains(&format!("iter_{v}")),
            "iteration {v}: hover must see latest name, got: {contents}"
        );

        let resp = server.references("iter.php", 1, 10, false).await;
        let refs = resp["result"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            refs.len(),
            1,
            "iteration {v}: expected 1 ref, got {}: {refs:?}",
            refs.len()
        );
    }
}

#[tokio::test]
async fn request_after_close_and_reopen_returns_fresh_data() {
    let mut server = TestServer::new().await;
    server
        .open("ro.php", "<?php\nfunction first(): void {}\n")
        .await;

    let uri = server.uri("ro.php");
    server
        .client()
        .notify(
            "textDocument/didClose",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;

    server
        .open("ro.php", "<?php\nfunction second(): void {}\n")
        .await;

    let resp = server.hover("ro.php", 1, 10).await;
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("second"),
        "hover after close+reopen must see new content, got: {contents}"
    );
    assert!(
        !contents.contains("first"),
        "hover must NOT see stale `first` from closed session, got: {contents}"
    );
}
