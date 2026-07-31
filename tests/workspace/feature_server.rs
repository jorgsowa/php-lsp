//! Server lifecycle and concurrency: initialize, shutdown, protocol stubs,
//! and sustained request interleaving under load.

// ── lifecycle ────────────────────────────────────────────────────────────────

// Every (feature-flag key, ServerCapabilities JSON field) pair.
use super::*;
use expect_test::expect;

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
    let (mut server, init_resp) = TestServer::new_with_options(serde_json::json!({})).await;
    assert!(
        !init_resp["result"]["capabilities"]["hoverProvider"].is_null(),
        "expected hoverProvider to be advertised: {init_resp:?}"
    );

    server
        .open("cap.php", "<?php\nfunction f(): void {}\n")
        .await;
    let resp = server.hover("cap.php", 1, 10).await;
    expect![[r#"
        ```php
        function f(): void
        ```"#]]
    .assert_eq(&render_hover(&resp));
}

/// `\` must be a completion trigger character: FQN completion after a bare
/// `\` (e.g. typing `\App\Models\`) is implemented (see completion/mod.rs's
/// sub-namespace completion), so clients that only auto-popup completion on
/// registered trigger characters need it advertised or the feature is
/// reachable only via manual invocation.
#[tokio::test]
async fn backslash_is_a_completion_trigger_character() {
    let (_, init_resp) = TestServer::new_with_options(serde_json::json!({})).await;
    let triggers = init_resp["result"]["capabilities"]["completionProvider"]["triggerCharacters"]
        .as_array()
        .expect("completionProvider should advertise triggerCharacters")
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        triggers.contains(&"\\"),
        "expected '\\\\' among trigger characters, got {triggers:?}"
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
        let out = render_hover(&resp);
        assert!(
            out.contains("pipeHover"),
            "hover content must stay correct across volley, got: {out}"
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
    expect![[r#"
        ```php
        function second(): void
        ```"#]]
    .assert_eq(&render_hover(&resp));
}

// ── $/cancelRequest ──────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_request_returns_request_cancelled_and_server_stays_alive() {
    let mut server = TestServer::new().await;
    // Enough open files that the references handler (workspace scan + mir
    // ingestion across several spawn_blocking await points) is still pending
    // when the cancel notification — sent in the very next frame — is
    // processed by tower-lsp's Cancellable layer.
    for i in 0..30 {
        let src =
            format!("<?php\nclass Worker{i} {{\n    public function doWork(): void {{}}\n}}\n");
        server.open(&format!("w{i}.php"), &src).await;
    }
    server
        .open(
            "cancel_main.php",
            "<?php\n$w = new Worker0();\n$w->doWork();\n",
        )
        .await;

    // $/cancelRequest is best-effort: when the request completes before the
    // cancel frame is processed, a normal result is a legitimate outcome and
    // the race simply wasn't observed. Retry until a cancellation lands —
    // what must NEVER happen is an error other than RequestCancelled.
    let uri = server.uri("cancel_main.php");
    let mut cancelled = false;
    for _ in 0..10 {
        let resp = server
            .request_then_cancel(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": 2, "character": 5 },
                    "context": { "includeDeclaration": true },
                }),
            )
            .await;
        match resp.get("error") {
            None => continue, // completed before the cancel was seen — retry
            Some(err) => {
                assert_eq!(
                    err["code"],
                    serde_json::json!(-32800),
                    "expected RequestCancelled (-32800), got: {resp}"
                );
                cancelled = true;
                break;
            }
        }
    }
    assert!(
        cancelled,
        "no attempt out of 10 produced a RequestCancelled response"
    );

    // A cancelled request must not wedge the server: a follow-up request on
    // the same document still answers normally.
    let hover = server.hover("cancel_main.php", 2, 5).await;
    assert!(
        hover.get("error").is_none(),
        "hover after cancellation must succeed, got: {hover}"
    );
}

// ── spawn_blocking consistency ────────────────────────────────────────────────

#[tokio::test]
async fn references_and_hover_concurrent_complete_without_deadlock() {
    // Two independent servers run references and hover at the same time,
    // exercising the spawn_blocking path in both handle_references (query.collect)
    // and handle_goto_definition (workspace_index_async). Neither request should
    // block the other or deadlock.
    let (mut srv_a, mut srv_b) = tokio::join!(TestServer::new(), TestServer::new());

    srv_a
        .open(
            "conc_a.php",
            "<?php\nfunction greet(string $name): string { return $name; }\ngreet('world');\n",
        )
        .await;
    srv_b
        .open(
            "conc_b.php",
            "<?php\nfunction greet(string $name): string { return $name; }\ngreet('world');\n",
        )
        .await;

    let (refs_resp, hover_resp) = tokio::join!(
        srv_a.references("conc_a.php", 1, 10, false),
        srv_b.hover("conc_b.php", 1, 10)
    );

    expect!["conc_a.php:2:0-2:5"].assert_eq(&render_locations(&refs_resp, &srv_a.uri("")));
    expect![[r#"
        ```php
        function greet(string $name): string
        ```"#]]
    .assert_eq(&render_hover(&hover_resp));
}

#[tokio::test]
async fn laravel_string_key_references_and_hover_concurrent_complete_without_deadlock() {
    // Laravel string-key references (env/config/route/view/translation
    // definitions) scan every workspace file's cached text on the blocking
    // pool, like the general references path above — never sequentially on
    // the tokio worker. Exercise it concurrently with an unrelated hover on
    // a second server the same way.
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::write(workspace.path().join(".env"), "APP_NAME=Test\n").unwrap();
    std::fs::write(
        workspace.path().join("a.php"),
        "<?php\n$x = env('APP_NAME');\n",
    )
    .unwrap();

    let (mut srv_a, mut srv_b) =
        tokio::join!(TestServer::with_root(workspace.path()), TestServer::new());
    srv_a.wait_for_index_ready().await;
    srv_a.open(".env", "APP_NAME=Test\n").await;
    srv_b
        .open(
            "conc_b.php",
            "<?php\nfunction greet(string $name): string { return $name; }\ngreet('world');\n",
        )
        .await;

    let (refs_resp, hover_resp) = tokio::join!(
        srv_a.references(".env", 0, 2, false),
        srv_b.hover("conc_b.php", 1, 10)
    );

    expect!["a.php:1:10-1:18"].assert_eq(&render_locations(&refs_resp, &srv_a.uri("")));
    expect![[r#"
        ```php
        function greet(string $name): string
        ```"#]]
    .assert_eq(&render_hover(&hover_resp));
}

// ── inline-blocking regression tests ────────────────────────────────────────
// Each handler below must offload its document-size synchronous work to
// `spawn_blocking` rather than running it directly on the async task that
// also reads stdin and writes stdout for the whole connection — see
// `TestClient::assert_stays_responsive` for why running it inline hangs
// every other in-flight request, not just the slow one.

// `assert_notification_stays_responsive`'s budget is a wall-clock timeout
// racing the server's own blocking work, unlike the ordering check
// `assert_stays_responsive` uses for request/response pairs. On the default
// current-thread runtime that race is meaningless: a `tokio::time::timeout`
// can only fire between polls, never partway through one, so a single
// non-yielding poll that runs longer than the budget simply delays the
// timer's own chance to fire until right when the blocking call itself
// returns — at that point both become ready at essentially the same
// instant and which one wins is coin-flip scheduling order, not the
// duration each actually took. `multi_thread` puts the timer on a real
// second OS thread so it fires independently of whatever the (possibly
// stuck) server task is doing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn did_open_stays_responsive_on_large_file() {
    // did_open parses the just-opened document synchronously to seed parse
    // diagnostics; a cache miss (this is the file's first open) must not run
    // that parse inline. Diagnostics are disabled so the test isolates the
    // parse itself from mir's separate (and much heavier) semantic analysis
    // pass, which would otherwise dominate wall time regardless of this bug.
    // 20000 generated classes measured ~43-53ms for the whole open+probe
    // exchange when parsed inline, vs. ~11-16ms when deferred via
    // spawn_blocking — comfortable margin either side of the 25ms budget.
    let (mut server, _init) = TestServer::new_with_options(serde_json::json!({
        "diagnostics": { "enabled": false }
    }))
    .await;
    let uri = server.uri("big_did_open.php");
    server
        .assert_notification_stays_responsive(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "php",
                    "version": 1,
                    "text": crate::common::fixture::large_php_source(20000),
                }
            }),
            std::time::Duration::from_millis(25),
        )
        .await;
}

#[tokio::test]
async fn prepare_rename_stays_responsive_for_keyword_shaped_identifier_on_large_file() {
    // `prepare_rename` only walks the whole AST when the word under the
    // cursor is shaped like a PHP keyword (`list`, `match`, ...) used
    // somewhere as an ordinary identifier — it must check every member-access
    // site in the document to tell whether this occurrence is one of them.
    // `list(...)` here is the builtin destructuring form (not a method call),
    // so the walk finds no match anywhere and must scan the entire tree.
    let mut source = crate::common::fixture::large_php_source(500);
    source.push_str("function useList() {\n    list($a, $b) = someFunc();\n    return $a;\n}\n");
    let list_line = source
        .lines()
        .position(|l| l.contains("list($a, $b)"))
        .expect("generated source must contain the list(...) line") as u32;
    let list_char = source
        .lines()
        .nth(list_line as usize)
        .unwrap()
        .find("list")
        .unwrap() as u32;

    let mut server = TestServer::new().await;
    server.open("big_prepare_rename.php", &source).await;
    let uri = server.uri("big_prepare_rename.php");
    server
        .assert_stays_responsive(
            "textDocument/prepareRename",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": list_line, "character": list_char },
            }),
        )
        .await;
}

#[tokio::test]
async fn linked_editing_range_stays_responsive_on_large_file() {
    let mut server = TestServer::new().await;
    server
        .open(
            "big_linked_editing.php",
            &crate::common::fixture::large_php_source(500),
        )
        .await;
    let uri = server.uri("big_linked_editing.php");
    server
        .assert_stays_responsive(
            "textDocument/linkedEditingRange",
            serde_json::json!({
                "textDocument": { "uri": uri },
                // Character 8 lands inside "GenClass0" on `class GenClass0`
                // (line 7 of `large_php_source`'s output).
                "position": { "line": 7, "character": 8 },
            }),
        )
        .await;
}

#[serial_test::serial(fake_external_formatter)]
#[tokio::test]
async fn formatting_stays_responsive_with_slow_external_formatter() {
    // format_document/format_range shell out to php-cs-fixer/phpcbf and
    // block on `wait_with_output()`. Neither tool is assumed installed in
    // dev/CI, so this test puts its own fake "php-cs-fixer" on PATH — a
    // script that sleeps briefly then reformats — to exercise the real
    // blocking-subprocess code path deterministically rather than the
    // near-instant "tool not found" fallback. #[serial] plus restoring PATH
    // on drop keeps this from racing or leaking into other tests that also
    // shell out.
    struct PathGuard(String);
    impl Drop for PathGuard {
        fn drop(&mut self) {
            unsafe { std::env::set_var("PATH", &self.0) };
        }
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("php-cs-fixer");
    std::fs::write(&script, "#!/bin/sh\nsleep 0.2\ncat\necho '// formatted'\n")
        .expect("write fake formatter");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake formatter");
    }

    let original_path = std::env::var("PATH").unwrap_or_default();
    let _restore_path = PathGuard(original_path.clone());
    unsafe {
        std::env::set_var("PATH", format!("{}:{original_path}", dir.path().display()));
    }

    let mut server = TestServer::new().await;
    server
        .open("fmt.php", "<?php\nfunction f(): void {}\n")
        .await;
    let uri = server.uri("fmt.php");
    server
        .assert_stays_responsive(
            "textDocument/formatting",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "options": { "tabSize": 4, "insertSpaces": true },
            }),
        )
        .await;
}

#[tokio::test]
async fn document_symbol_stays_responsive_on_large_file() {
    let mut server = TestServer::new().await;
    server
        .open(
            "big_doc_symbol.php",
            &crate::common::fixture::large_php_source(500),
        )
        .await;
    let uri = server.uri("big_doc_symbol.php");
    server
        .assert_stays_responsive(
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;
}

#[tokio::test]
async fn folding_range_stays_responsive_on_large_file() {
    let mut server = TestServer::new().await;
    server
        .open(
            "big_folding.php",
            &crate::common::fixture::large_php_source(500),
        )
        .await;
    let uri = server.uri("big_folding.php");
    server
        .assert_stays_responsive(
            "textDocument/foldingRange",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;
}

#[tokio::test]
async fn document_link_stays_responsive_on_large_file() {
    let mut server = TestServer::new().await;
    server
        .open(
            "big_doc_link.php",
            &crate::common::fixture::large_php_source(500),
        )
        .await;
    let uri = server.uri("big_doc_link.php");
    server
        .assert_stays_responsive(
            "textDocument/documentLink",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;
}

#[tokio::test]
async fn semantic_tokens_full_stays_responsive_on_large_file() {
    let mut server = TestServer::new().await;
    server
        .open("big_semtok_full.php", &crate::common::fixture::large_php_source(500))
        .await;
    let uri = server.uri("big_semtok_full.php");
    server
        .assert_stays_responsive(
            "textDocument/semanticTokens/full",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await;
}

#[tokio::test]
async fn semantic_tokens_range_stays_responsive_on_large_file() {
    let mut server = TestServer::new().await;
    server
        .open("big_semtok_range.php", &crate::common::fixture::large_php_source(500))
        .await;
    let uri = server.uri("big_semtok_range.php");
    server
        .assert_stays_responsive(
            "textDocument/semanticTokens/range",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 50, "character": 0 },
                },
            }),
        )
        .await;
}

#[tokio::test]
async fn semantic_tokens_full_delta_stays_responsive_on_large_file() {
    let mut server = TestServer::new().await;
    server
        .open("big_semtok_delta.php", &crate::common::fixture::large_php_source(500))
        .await;
    let uri = server.uri("big_semtok_delta.php");
    server
        .assert_stays_responsive(
            "textDocument/semanticTokens/full/delta",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "previousResultId": "nonexistent",
            }),
        )
        .await;
}
