//! Incremental `didChange` correctness: cache invalidation, cross-file republish,
//! burst debouncing, and reopen stability.

use super::*;

use expect_test::expect;

fn render_def_location(resp: &serde_json::Value, root_uri: &str) -> String {
    common::render_locations(resp, root_uri)
}

#[tokio::test]
async fn incremental_ranged_change_applies_to_buffer() {
    let mut server = TestServer::new().await;
    server
        .open(
            "inc.php",
            "<?php\nfunction oldName(): void {}\noldName();\n",
        )
        .await;

    // Replace `oldName` with `newName` in both the declaration (line 1,
    // cols 9..16) and the call (line 2, cols 0..7) — two ranged changes in
    // one didChange, second range valid against the result of the first.
    server
        .change_incremental(
            "inc.php",
            2,
            &[(1, 9, 1, 16, "newName"), (2, 0, 2, 7, "newName")],
        )
        .await;

    let out = server.hover("inc.php", 2, 1).await;
    let rendered = common::render_hover(&out);
    expect![[r#"
        ```php
        function newName(): void
        ```"#]]
    .assert_eq(&rendered);
}

#[tokio::test]
async fn incremental_insertion_and_newline_deletion() {
    let mut server = TestServer::new().await;
    let root_uri = server.uri("");
    server
        .open("ins.php", "<?php\nfunction target(): void {}\n\n\n")
        .await;

    // Insert a call on line 2 (empty), then delete the now-trailing blank
    // line 3 by removing its newline span.
    server
        .change_incremental("ins.php", 2, &[(2, 0, 2, 0, "target();"), (3, 0, 4, 0, "")])
        .await;

    let resp = server.definition("ins.php", 2, 2).await;
    let out = render_def_location(&resp, &root_uri);
    expect!["ins.php:1:9-1:15"].assert_eq(&out);
}

#[tokio::test]
async fn incremental_change_with_multibyte_text() {
    let mut server = TestServer::new().await;
    server
        .open(
            "emoji.php",
            "<?php\n// 😀 marker\nfunction greeter(): string { return ''; }\n",
        )
        .await;

    // Replace `''` (line 2, cols 36..38) with a multibyte literal; the range
    // is in UTF-16 columns and must land after the emoji comment untouched.
    server
        .change_incremental("emoji.php", 2, &[(2, 36, 2, 38, "'héllo'")])
        .await;

    let out = server.hover("emoji.php", 2, 10).await;
    let rendered = common::render_hover(&out);
    expect![[r#"
        ```php
        function greeter(): string
        ```"#]]
    .assert_eq(&rendered);
}

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

    let out = server.hover("edit.php", 1, 10).await;
    let rendered = common::render_hover(&out);
    expect![[r#"
        ```php
        function greeter(string $name): string
        ```"#]]
    .assert_eq(&rendered);
}

#[tokio::test]
async fn definition_cache_is_invalidated_after_didchange() {
    let mut server = TestServer::new().await;
    let root_uri = server.uri("");
    server
        .open(
            "ren.php",
            "<?php\nfunction oldName(): void {}\noldName();\n",
        )
        .await;

    let resp_v1 = server.definition("ren.php", 2, 1).await;
    let out_v1 = render_def_location(&resp_v1, &root_uri);
    expect!["ren.php:1:9-1:16"].assert_eq(&out_v1);

    server
        .change(
            "ren.php",
            2,
            "<?php\n\nfunction newName(): void {}\nnewName();\n",
        )
        .await;

    let resp_v2 = server.definition("ren.php", 3, 1).await;
    let out_v2 = render_def_location(&resp_v2, &root_uri);
    // Cache must be invalidated: location moves to line 2 (added blank line)
    expect!["ren.php:2:9-2:16"].assert_eq(&out_v2);
}

#[tokio::test]
async fn references_reflect_didchange_additions_and_removals() {
    let mut server = TestServer::new().await;
    server
        .open("refs.php", "<?php\nfunction target(): void {}\ntarget();\n")
        .await;

    server
        .change(
            "refs.php",
            2,
            "<?php\nfunction target(): void {}\ntarget();\ntarget();\n",
        )
        .await;

    let resp = server.references("refs.php", 1, 9, false).await;
    let root = server.uri("");
    expect![[r#"
        refs.php:2:0-2:6
        refs.php:3:0-3:6"#]]
    .assert_eq(&render_locations(&resp, &root));

    server
        .change(
            "refs.php",
            3,
            "<?php\nfunction target(): void {}\ntarget();\n",
        )
        .await;

    let resp = server.references("refs.php", 1, 9, false).await;
    expect!["refs.php:2:0-2:6"].assert_eq(&render_locations(&resp, &root));
}

#[tokio::test]
async fn diagnostics_replaced_not_appended_on_didchange() {
    let mut server = TestServer::new().await;
    let notif = server.open("d.php", "<?php\nbroken(;\n").await;
    expect![[r#"
        1:0-1:7 [1] UndefinedFunction: Function broken() is not defined
        1:7-1:8 [1] SyntaxError: expected ')', found ';'
        1:7-1:8 [1] SyntaxError: expected expression"#]]
    .assert_eq(&render_diagnostics_notification(&notif));

    let notif = server.change("d.php", 2, "<?php\n").await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn cross_file_diagnostics_refresh_on_next_didchange() {
    let mut server = TestServer::new().await;
    server.open("dep.php", "<?php\nclass Widget {}\n").await;
    let notif = server.open("user.php", "<?php\n$w = new Widget();\n").await;
    // Widget is defined — no error initially.
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif));

    server
        .change("dep.php", 2, "<?php\nclass Gadget {}\n")
        .await;

    let notif = server
        .change("user.php", 2, "<?php\n$w = new Widget();\n")
        .await;
    // Widget renamed to Gadget in dep.php; user.php must report UndefinedClass.
    expect!["1:9-1:15 [1] UndefinedClass: Class Widget does not exist"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn cross_file_diagnostics_republish_on_dependency_change() {
    let mut server = TestServer::new().await;
    server.open("dep2.php", "<?php\nclass Widget2 {}\n").await;
    server
        .open("user2.php", "<?php\n$w = new Widget2();\n")
        .await;

    // Drain the dependents-sweep that user2's own open triggered in the
    // background, so its leftover salsa snapshot can't stall the change
    // below's synchronous `set_open_text` write (see
    // cross_file_republish_fans_out_to_multiple_dependents).
    let _ = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(200))
        .await;

    server
        .change("dep2.php", 2, "<?php\nclass Gadget2 {}\n")
        .await;

    let uri = server.uri("user2.php");
    let notif = server.client().wait_for_diagnostics(&uri).await;
    // Widget2 renamed to Gadget2; user2.php proactively republished.
    expect!["1:9-1:16 [1] UndefinedClass: Class Widget2 does not exist"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn true_burst_didchange_converges_to_final_text() {
    let mut server = TestServer::new().await;
    server.open("burst.php", "<?php\n").await;

    let uri = server.uri("burst.php");
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

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if std::time::Instant::now() >= deadline {
            panic!("timed out waiting for burst to settle");
        }
        let resp = server.hover("burst.php", 1, 10).await;
        let contents = resp["result"]["contents"].to_string();
        if contents.contains("f6") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

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
    expect![[r#"
        reopen.php:1:9-1:13
        reopen.php:2:0-2:4"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}

#[tokio::test]
async fn cross_file_diagnostic_clears_when_dependency_opened() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("user_open.php", "<?php\n$w = new ProvidedClass();\n")
        .await;
    // ProvidedClass not yet defined.
    expect!["1:9-1:22 [1] UndefinedClass: Class ProvidedClass does not exist"]
        .assert_eq(&render_diagnostics_notification(&notif));

    server
        .open("provider.php", "<?php\nclass ProvidedClass {}\n")
        .await;

    let user_uri = server.uri("user_open.php");
    let notif = server.client().wait_for_diagnostics(&user_uri).await;
    // ProvidedClass now defined; error cleared.
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif));
}

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

    let _ = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(200))
        .await;

    server
        .change("dep_fan.php", 2, "<?php\nclass FanGadget {}\n")
        .await;

    let u1 = server.uri("u1_fan.php");
    let u2 = server.uri("u2_fan.php");
    let notifs = server
        .client()
        .wait_for_diagnostics_multi(&[&u1, &u2])
        .await;

    // Both files must receive diagnostics
    assert!(
        notifs.contains_key(&u1),
        "u1_fan.php did not receive publishDiagnostics"
    );
    assert!(
        notifs.contains_key(&u2),
        "u2_fan.php did not receive publishDiagnostics"
    );

    for (label, uri) in [("u1", &u1), ("u2", &u2)] {
        let notif = notifs
            .get(uri)
            .unwrap_or_else(|| panic!("{label} did not receive publishDiagnostics"));
        expect!["1:9-1:18 [1] UndefinedClass: Class FanWidget does not exist"]
            .assert_eq(&render_diagnostics_notification(notif));
    }
}

#[tokio::test]
async fn cross_file_republish_skips_closed_files() {
    let mut server = TestServer::new().await;
    server
        .open("dep_closed.php", "<?php\nclass ClosedDep {}\n")
        .await;
    server
        .open("user_closed.php", "<?php\n$w = new ClosedDep();\n")
        .await;

    let user_uri = server.uri("user_closed.php");
    server.close("user_closed.php").await;
    let _ = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(200))
        .await;

    server
        .change("dep_closed.php", 2, "<?php\nclass ClosedDepRenamed {}\n")
        .await;

    let seen = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(300))
        .await;
    assert!(
        !seen.iter().any(|u| u == &user_uri),
        "closed file received an unexpected publishDiagnostics: {seen:?}"
    );
}

#[tokio::test]
async fn cross_file_republish_uses_empty_array_for_clean_dependent() {
    // `clean_b` references `aa()`, which `clean_a` does not define yet — so
    // `clean_b` opens dirty. Defining `aa()` in `clean_a` must republish
    // `clean_b`, and the diagnostics field must be an *empty array*, not an
    // omitted field (LSP requires the field even when there are no issues).
    // A dependent whose diagnostics are unchanged is deliberately NOT
    // republished — the sweep dedups no-op publishes.
    let mut server = TestServer::new().await;
    server
        .open("clean_a.php", "<?php\nfunction extra(): void {}\n")
        .await;
    let notif = server.open("clean_b.php", "<?php\naa();\n").await;
    expect!["1:0-1:4 [1] UndefinedFunction: Function aa() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));

    // clean_b's own open triggers a dependents-sweep that re-analyzes
    // clean_a in the background; drain it before editing clean_a so that
    // sweep's leftover salsa snapshot can't stall the edit's synchronous
    // `set_open_text` write (see cross_file_republish_fans_out_to_multiple_dependents).
    let _ = server
        .client()
        .drain_publish_diagnostics_uris(tokio::time::Duration::from_millis(200))
        .await;

    server
        .change(
            "clean_a.php",
            2,
            "<?php\nfunction aa(): void {}\nfunction extra(): void {}\n",
        )
        .await;

    let b_uri = server.uri("clean_b.php");
    let notif = server.client().wait_for_diagnostics(&b_uri).await;
    assert!(
        notif["params"]["diagnostics"].is_array(),
        "diagnostics field must be present as an array"
    );
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn cross_file_republish_preserves_dependent_parse_errors() {
    // `broken.php` has a parse error AND references `Triggered`, which is
    // about to be defined by `trigger.php`. When the dependent is
    // republished, the parse error must survive (we merge LSP-side parse
    // diagnostics with mir's semantic issues).
    let mut server = TestServer::new().await;
    let notif = server
        .open("broken.php", "<?php\nnew Triggered();\nbroken(;\n")
        .await;
    // Initially: UndefinedClass for Triggered + parse errors from broken(;
    expect![[r#"
        1:4-1:13 [1] UndefinedClass: Class Triggered does not exist
        2:0-2:7 [1] UndefinedFunction: Function broken() is not defined
        2:7-2:8 [1] SyntaxError: expected ')', found ';'
        2:7-2:8 [1] SyntaxError: expected expression"#]]
    .assert_eq(&render_diagnostics_notification(&notif));

    server
        .open("trigger.php", "<?php\nclass Triggered {}\n")
        .await;

    let broken_uri = server.uri("broken.php");
    let notif = server.client().wait_for_diagnostics(&broken_uri).await;
    // UndefinedClass clears once Triggered is defined; parse errors survive.
    expect![[r#"
        2:7-2:8 [1] SyntaxError: expected ')', found ';'
        2:7-2:8 [1] SyntaxError: expected expression"#]]
    .assert_eq(&render_diagnostics_notification(&notif));
}

// ── snapshot_query retry reduction ───────────────────────────────────────────

#[tokio::test]
async fn rapid_concurrent_edits_and_queries_no_panic() {
    // Interleave rapid didChange updates with hover and references requests.
    // This exercises the snapshot_query single-retry-then-lock path — if it
    // deadlocks or panics, the test will hang or fail.
    let mut server = TestServer::new().await;
    server
        .open(
            "rapid.php",
            "<?php\nfunction target(): int { return 0; }\ntarget();\n",
        )
        .await;

    for v in 2..=12 {
        let src = format!("<?php\nfunction target(): int {{ return {v}; }}\ntarget();\n");
        server.change("rapid.php", v, &src).await;

        let hover = server.hover("rapid.php", 1, 10).await;
        assert!(
            hover["error"].is_null(),
            "hover must not error during rapid edits (iteration {v}): {hover:?}"
        );

        let refs = server.references("rapid.php", 1, 10, false).await;
        assert!(
            refs["error"].is_null(),
            "references must not error during rapid edits (iteration {v}): {refs:?}"
        );
    }
}

// ── owned_program cache ───────────────────────────────────────────────────────

#[tokio::test]
async fn declaration_change_in_one_file_does_not_break_sibling_hover() {
    // When file A's declaration changes (bumping decl_version), file B's
    // owned_program_cache entry should be reused since B's source is unchanged.
    // We verify this indirectly: hover on B must return correct results both
    // before and after the sibling declaration edit, with no panics.
    let mut server = TestServer::new().await;

    server.open("sib_a.php", "<?php\nclass Alpha {}\n").await;
    server
        .open(
            "sib_b.php",
            "<?php\nfunction computeValue(int $x): int { return $x * 2; }\n",
        )
        .await;

    let hover_before = server.hover("sib_b.php", 1, 10).await;
    expect![[r#"
        ```php
        function computeValue(int $x): int
        ```"#]]
    .assert_eq(&render_hover(&hover_before));

    // Edit sib_a.php — this is a declaration change that bumps decl_version
    // and would previously force a fresh to_owned_program() for sib_b.
    server
        .change("sib_a.php", 2, "<?php\nclass AlphaRenamed {}\n")
        .await;

    let hover_after = server.hover("sib_b.php", 1, 10).await;
    expect![[r#"
        ```php
        function computeValue(int $x): int
        ```"#]]
    .assert_eq(&render_hover(&hover_after));
}
