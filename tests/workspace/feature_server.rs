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

// Unlike the request/response pairs above, a notification has no response to
// order a probe against, so these use the server's debug gate instead of any
// wall-clock measurement — see `TestClient::assert_notification_stays_responsive`
// (and the flaky-budget history in its doc comment) for the mechanism. The
// payload can therefore be tiny: the gate, not document size, is what keeps
// the handler's work observably in flight.
//
// `multi_thread` isn't needed for the pass path (deterministic on any
// runtime); it keeps the client task alive when a regressed inline handler
// freezes the server's serve future, so the failure is attributed to
// blocking rather than to a gate-wiring problem.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn did_open_stays_responsive_while_parse_is_in_flight() {
    // did_open parses the just-opened document to seed parse diagnostics; a
    // cache miss (the file's first open) runs a full parse, which must stay
    // off the serve future. Diagnostics are disabled to pin this test to the
    // parse path rather than mir's separate semantic analysis pass.
    let (mut server, _init) = TestServer::new_with_options(serde_json::json!({
        "diagnostics": { "enabled": false }
    }))
    .await;
    let uri = server.uri("gated_did_open.php");
    server
        .assert_notification_stays_responsive(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": "php",
                    "version": 1,
                    "text": "<?php\nfunction gated(): int { return 1; }\n",
                }
            }),
            php_lsp::backend::debug_gate::GATE_DID_OPEN_PARSE,
        )
        .await;
}

/// Opens `count` sizeable, self-contained documents plus one "target"
/// document referencing a name (`undefinedGoal`) that isn't declared
/// anywhere, so `goto_type_definition`'s open-doc AST scan must walk every
/// one of them before concluding there's no match.
async fn open_many_docs_with_unresolved_target(server: &mut TestServer, count: usize) -> String {
    for i in 0..count {
        server
            .open(
                &format!("noise{i}.php"),
                &crate::common::fixture::large_php_source(30),
            )
            .await;
    }
    server
        .open(
            "goto_target.php",
            "<?php\nfunction useIt(): void {\n    undefinedGoal();\n}\n",
        )
        .await;
    server.uri("goto_target.php")
}

// Note: `goto_declaration` has the identical inline-scan pattern (walks
// every open doc's AST via `resolve_declaration` with no spawn_blocking),
// but its per-doc cost is dominated by cheap top-level name comparisons —
// empirically, even 1500 open documents didn't produce an observable
// ordering effect here, unlike goto_type_definition below. The source fix
// is kept for architectural consistency (and headroom against future
// per-doc cost growth), but isn't paired with its own responsiveness test
// since one can't be constructed reliably at a practical scale.

#[tokio::test]
async fn goto_type_definition_stays_responsive_with_many_open_documents() {
    let mut server = TestServer::new().await;
    let uri = open_many_docs_with_unresolved_target(&mut server, 150).await;
    server
        .assert_stays_responsive(
            "textDocument/typeDefinition",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 6 },
            }),
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

#[cfg(unix)]
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
        .open(
            "big_semtok_full.php",
            &crate::common::fixture::large_php_source(500),
        )
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
        .open(
            "big_semtok_range.php",
            &crate::common::fixture::large_php_source(500),
        )
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn did_save_stays_responsive_while_diagnostics_recompute_is_in_flight() {
    // `did_save` must recompute diagnostics (including the mir semantic pass)
    // off the async runtime worker, same as `did_open`/`did_change`'s shared
    // `publish_with_dependents` path. Gate-based like the did_open test
    // above, so the file's size and cache warmth are irrelevant.
    let mut server = TestServer::new().await;
    server
        .open(
            "gated_save.php",
            "<?php\nfunction saved(): int { return 1; }\n",
        )
        .await;
    let uri = server.uri("gated_save.php");
    server
        .assert_notification_stays_responsive(
            "textDocument/didSave",
            serde_json::json!({ "textDocument": { "uri": uri } }),
            php_lsp::backend::debug_gate::GATE_DID_SAVE_DIAGNOSTICS,
        )
        .await;
}

// `did_change_watched_files` parses each CREATED/CHANGED file and batches
// that into one `spawn_blocking` call rather than N (same motivation as the
// did_change debounce comment above). The loop itself awaits
// `tokio::fs::read_to_string` per file before parsing it, so a wall-clock or
// pure-ordering probe couldn't previously pin the batch's own inline-vs-
// spawn_blocking behavior at any practical file count/size — the gate below
// sidesteps that entirely, same as did_open/did_save.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn did_change_watched_files_stays_responsive_while_batch_parse_is_in_flight() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let mut server = TestServer::with_root(workspace.path()).await;
    server.wait_for_index_ready().await;
    server.write_file("gated_watched.php", "<?php\nclass GatedWatched {}\n");
    let uri = server.uri("gated_watched.php");
    server
        .assert_notification_stays_responsive(
            "workspace/didChangeWatchedFiles",
            serde_json::json!({ "changes": [{ "uri": uri, "type": 1 }] }),
            php_lsp::backend::debug_gate::GATE_DID_CHANGE_WATCHED_FILES,
        )
        .await;
}

/// This test confirms the batched rewrite still indexes every file in a
/// single bulk notification, mixing a CREATE and a DELETE in one batch to
/// exercise the enum's ordering.
#[tokio::test]
async fn did_change_watched_files_bulk_batch_indexes_every_file() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let mut server = TestServer::with_root(workspace.path()).await;
    server.wait_for_index_ready().await;

    server.write_file("ToDelete.php", "<?php\nclass ToDelete {}\n");
    server
        .did_change_watched_files(vec![(server.uri("ToDelete.php"), 1)])
        .await;
    server
        .wait_until_symbol_present("ToDelete", std::time::Duration::from_secs(3))
        .await;
    std::fs::remove_file(workspace.path().join("ToDelete.php")).unwrap();

    let mut changes = vec![(server.uri("ToDelete.php"), 3)]; // DELETED
    for i in 0..50 {
        let path = format!("Bulk{i}.php");
        server.write_file(&path, &format!("<?php\nclass Bulk{i} {{}}\n"));
        changes.push((server.uri(&path), 1)); // CREATED
    }
    server.did_change_watched_files(changes).await;

    server
        .wait_until_symbol_present("Bulk49", std::time::Duration::from_secs(3))
        .await;
    for i in 0..50 {
        let found = server.snapshot_workspace_symbols(&format!("Bulk{i}")).await;
        assert!(
            found.contains(&format!("Bulk{i}")),
            "Bulk{i} missing from workspace symbols after bulk batch: {found}"
        );
    }
    let deleted = server.snapshot_workspace_symbols("ToDelete").await;
    assert!(
        deleted.contains("no symbols"),
        "ToDelete must be gone after DELETED in the same batch, got: {deleted}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn will_rename_files_stays_responsive_while_use_edit_batch_is_in_flight() {
    // Renaming a class parses every file that imports it (to rewrite `use`
    // lines) in one batched `spawn_blocking` call. That closure sits
    // between two other `.await` points in the same handler (the psr4-map
    // load and the reference-lookup spawn_blocking below it), so a plain
    // ordering probe against the handler's own response doesn't reliably
    // pin an inline regression in just this one hop — the runtime can still
    // interleave the probe through the handler's other await points even
    // when this specific hop runs inline. The gate sidesteps that, same
    // rationale as the notification-flavored tests above; a single
    // importing file is all that's needed since the gate — not file count —
    // is what proves the work is in flight.
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(
        workspace.path().join("composer.json"),
        r#"{"autoload": {"psr-4": {"": "src/"}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(
        workspace.path().join("src/Target.php"),
        "<?php\nclass Target {}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("src/Importer.php"),
        "<?php\nuse Target;\n\nclass Importer\n{\n    public function f(): Target\n    {\n        return new Target();\n    }\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(workspace.path()).await;
    server.wait_for_index_ready().await;
    let old_uri = server.uri("src/Target.php");
    let new_uri = server.uri("src/Renamed.php");
    server
        .assert_request_stays_responsive_via_gate(
            "workspace/willRenameFiles",
            serde_json::json!({
                "files": [{ "oldUri": old_uri, "newUri": new_uri }],
            }),
            php_lsp::backend::debug_gate::GATE_WILL_RENAME_FILES,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn will_delete_files_stays_responsive_while_use_edit_batch_is_in_flight() {
    // Same rationale as will_rename_files above: deleting a class parses
    // every file that imports it, in one batched spawn_blocking call, but
    // that closure sits between other await points in the same handler.
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(
        workspace.path().join("composer.json"),
        r#"{"autoload": {"psr-4": {"": "src/"}}}"#,
    )
    .unwrap();
    std::fs::create_dir_all(workspace.path().join("src")).unwrap();
    std::fs::write(
        workspace.path().join("src/Target.php"),
        "<?php\nclass Target {}\n",
    )
    .unwrap();
    std::fs::write(
        workspace.path().join("src/Importer.php"),
        "<?php\nuse Target;\n\nclass Importer\n{\n    public function f(): Target\n    {\n        return new Target();\n    }\n}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(workspace.path()).await;
    server.wait_for_index_ready().await;
    let uri = server.uri("src/Target.php");
    server
        .assert_request_stays_responsive_via_gate(
            "workspace/willDeleteFiles",
            serde_json::json!({ "files": [{ "uri": uri }] }),
            php_lsp::backend::debug_gate::GATE_WILL_DELETE_FILES,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prepare_call_hierarchy_stays_responsive_while_indexed_lookup_is_in_flight() {
    // A `decls_by_name` miss falls through to a workspace-wide trait-alias
    // scan; that whole indexed lookup must run off the async runtime worker.
    let mut server = TestServer::new().await;
    server
        .open(
            "gated_call_hierarchy.php",
            "<?php\nfunction gatedCallable(): void {}\ngatedCallable();\n",
        )
        .await;
    let uri = server.uri("gated_call_hierarchy.php");
    server
        .assert_request_stays_responsive_via_gate(
            "textDocument/prepareCallHierarchy",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 10 },
            }),
            php_lsp::backend::debug_gate::GATE_PREPARE_CALL_HIERARCHY,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn selection_range_stays_responsive_while_walk_is_in_flight() {
    // selection_range walks every top-level statement per requested
    // position to find the chain containing it; that walk must run off
    // the async runtime worker.
    let mut server = TestServer::new().await;
    server
        .open(
            "gated_selection_range.php",
            "<?php\nfunction gated(): int { return 1; }\n",
        )
        .await;
    let uri = server.uri("gated_selection_range.php");
    server
        .assert_request_stays_responsive_via_gate(
            "textDocument/selectionRange",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "positions": [{ "line": 1, "character": 25 }],
            }),
            php_lsp::backend::debug_gate::GATE_SELECTION_RANGE,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goto_implementation_stays_responsive_while_method_decl_check_is_in_flight() {
    // Disambiguating a class name from a same-named method walks every
    // member of every class in the document; that check must run off the
    // async runtime worker.
    let mut server = TestServer::new().await;
    server
        .open(
            "gated_goto_impl.php",
            "<?php\ninterface Gated {\n    public function work(): void;\n}\n",
        )
        .await;
    let uri = server.uri("gated_goto_impl.php");
    server
        .assert_request_stays_responsive_via_gate(
            "textDocument/implementation",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": 1, "character": 11 },
            }),
            php_lsp::backend::debug_gate::GATE_GOTO_IMPLEMENTATION,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rename_stays_responsive_while_property_decl_check_is_in_flight() {
    // Renaming a `$variable` first checks whether the cursor is on a
    // property declaration or promoted constructor param (both walk every
    // class member in the document) before falling back to the
    // single-document scope walker; both must run off the async runtime
    // worker.
    let mut server = TestServer::new().await;
    server
        .open(
            "gated_rename.php",
            "<?php\nfunction gated(): void {\n    $gatedVar = 1;\n}\n",
        )
        .await;
    let uri = server.uri("gated_rename.php");
    server
        .assert_request_stays_responsive_via_gate(
            "textDocument/rename",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": 2, "character": 8 },
                "newName": "$renamedVar",
            }),
            php_lsp::backend::debug_gate::GATE_RENAME_VARIABLE,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_resolve_stays_responsive_while_all_indexes_lookup_is_in_flight() {
    // A cold workspace-index rebuild walks every `FileIndex` in the
    // workspace, and the signature/doc lookup itself scans every indexed
    // file for a name match; both must run off the async runtime worker.
    let mut server = TestServer::new().await;
    server
        .open(
            "gated_completion_resolve.php",
            "<?php\nfunction gatedResolveFn(): void {}\ngatedResolveFn(",
        )
        .await;
    let resp = server
        .completion("gated_completion_resolve.php", 2, 15)
        .await;
    let items: Vec<_> = resp["result"]
        .as_array()
        .or_else(|| resp["result"]["items"].as_array())
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let item = items
        .into_iter()
        .find(|i| i["label"].as_str() == Some("gatedResolveFn"))
        .expect("gatedResolveFn completion item not found");

    server
        .assert_request_stays_responsive_via_gate(
            "completionItem/resolve",
            item,
            php_lsp::backend::debug_gate::GATE_COMPLETION_RESOLVE,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inlay_hint_resolve_stays_responsive_while_all_indexes_lookup_is_in_flight() {
    // Same whole-workspace-index scan as `completionItem/resolve`; must run
    // off the async runtime worker.
    let mut server = TestServer::new().await;
    server
        .open(
            "gated_inlay_hint_resolve.php",
            "<?php\nfunction gatedInlayFn(string $name): void {}\ngatedInlayFn(\"x\");\n",
        )
        .await;
    let resp = server
        .inlay_hints("gated_inlay_hint_resolve.php", 0, 0, 3, 0)
        .await;
    let hints: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let hint = hints
        .into_iter()
        .find(|h| {
            h["data"]["php_lsp_fn"].as_str() == Some("gatedInlayFn") && h["tooltip"].is_null()
        })
        .expect("gatedInlayFn inlay hint not found");

    server
        .assert_request_stays_responsive_via_gate(
            "inlayHint/resolve",
            hint,
            php_lsp::backend::debug_gate::GATE_INLAY_HINT_RESOLVE,
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_type_formatting_stays_responsive_while_scan_is_in_flight() {
    // Typing `}` builds a whole-document char vec plus string-literal mask
    // to find the matching brace; that fires on every keystroke, so it must
    // run off the async runtime worker.
    let mut server = TestServer::new().await;
    server
        .open(
            "gated_on_type_format.php",
            "<?php\nfunction gated(): void {\n    echo 1;\n}\n",
        )
        .await;
    let uri = server.uri("gated_on_type_format.php");
    server
        .assert_request_stays_responsive_via_gate(
            "textDocument/onTypeFormatting",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": 3, "character": 1 },
                "ch": "}",
                "options": { "tabSize": 4, "insertSpaces": true },
            }),
            php_lsp::backend::debug_gate::GATE_ON_TYPE_FORMATTING,
        )
        .await;
}

#[tokio::test]
async fn semantic_tokens_full_delta_stays_responsive_on_large_file() {
    let mut server = TestServer::new().await;
    server
        .open(
            "big_semtok_delta.php",
            &crate::common::fixture::large_php_source(500),
        )
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
