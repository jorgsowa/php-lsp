//! Incremental `didChange` correctness: cache invalidation, cross-file republish,
//! burst debouncing, and reopen stability.

mod common;

use common::TestServer;

fn has_code(notif: &serde_json::Value, code: &str) -> bool {
    notif["params"]["diagnostics"]
        .as_array()
        .map(|arr| arr.iter().any(|d| d["code"].as_str() == Some(code)))
        .unwrap_or(false)
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

    let resp = server.hover("edit.php", 1, 10).await;
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("greeter") && contents.contains("string"),
        "hover after didChange must see the new function signature, got: {contents}"
    );
}

#[tokio::test]
async fn definition_cache_is_invalidated_after_didchange() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ren.php",
            "<?php\nfunction oldName(): void {}\noldName();\n",
        )
        .await;

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

    server
        .change(
            "ren.php",
            2,
            "<?php\n\nfunction newName(): void {}\nnewName();\n",
        )
        .await;

    let resp = server.definition("ren.php", 3, 1).await;
    let result = &resp["result"];
    assert!(!result.is_null(), "newName() must resolve after didChange");
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    assert_eq!(
        loc["range"]["start"]["line"].as_u64().unwrap(),
        2,
        "expected V2 line (2), stale V1 result would be line 1"
    );
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
    let refs = resp["result"].as_array().expect("references array");
    assert_eq!(
        refs.len(),
        2,
        "expected both call sites after edit: {refs:?}"
    );

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

    server
        .change("dep.php", 2, "<?php\nclass Gadget {}\n")
        .await;

    let notif = server
        .change("user.php", 2, "<?php\n$w = new Widget();\n")
        .await;
    assert!(
        has_code(&notif, "UndefinedClass"),
        "after renaming Widget→Gadget in dep.php, user.php must report UndefinedClass: {:?}",
        notif["params"]["diagnostics"]
    );
}

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

    let uri = server.uri("user2.php");
    let notif = server.client().wait_for_diagnostics(&uri).await;
    assert!(
        has_code(&notif, "UndefinedClass"),
        "expected proactive UndefinedClass on user2.php after dependency edit"
    );
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
    let refs = resp["result"].as_array().expect("references array");
    assert_eq!(
        refs.len(),
        2,
        "expected declaration + 1 call, not duplicates after reopen: {refs:?}"
    );
}

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
    let mut server = TestServer::new().await;
    server
        .open("clean_a.php", "<?php\nfunction aa(): void {}\n")
        .await;
    server
        .open("clean_b.php", "<?php\nfunction bb(): void {}\n")
        .await;

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
