//! Document lifecycle: didClose, didSave, willSaveWaitUntil, didChange,
//! and basic endpoint wiring (documentLink, inlineValue).

mod common;

use common::TestServer;

// --- did_close ---

#[tokio::test]
async fn did_close_clears_diagnostics() {
    let mut server = TestServer::new().await;
    let uri = server.uri("close_test.php");

    let open_notif = server.open("close_test.php", "<?php function() {}\n").await;
    assert!(
        !open_notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected parse errors before close: {open_notif:?}"
    );

    server.close("close_test.php").await;
    let close_notif = server.client().wait_for_diagnostics(&uri).await;
    assert!(
        close_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "expected empty diagnostics after close: {close_notif:?}"
    );
}

#[tokio::test]
async fn did_close_unopened_does_not_crash() {
    let mut server = TestServer::new().await;
    let uri = server.uri("never_opened.php");

    server.close("never_opened.php").await;
    let notif = server.client().wait_for_diagnostics(&uri).await;
    assert!(
        notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "expected empty diagnostics for never-opened file: {notif:?}"
    );
}

// --- did_save ---

#[tokio::test]
async fn did_save_republishes_empty_diagnostics_for_clean_file() {
    let mut server = TestServer::new().await;
    server.open("save_clean.php", "<?php\n").await;

    let save_notif = server.save("save_clean.php").await;
    assert!(
        save_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "expected no diagnostics after save of clean file: {save_notif:?}"
    );
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
    assert!(
        !open_notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected duplicate-declaration diagnostic on open: {open_notif:?}"
    );

    let save_notif = server.save("save_dup.php").await;
    assert!(
        save_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .len()
            >= 1,
        "expected >=1 diagnostic after save with duplicate functions: {save_notif:?}"
    );
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
    assert!(
        !open_notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected semantic diagnostic on open: {open_notif:?}"
    );

    let save_notif = server.save("save_semantic.php").await;
    assert!(
        !save_notif["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty(),
        "did_save must republish semantic diagnostics, got empty list: {save_notif:?}"
    );
}

// --- willSaveWaitUntil ---

#[tokio::test]
async fn will_save_wait_until_returns_null_or_empty_for_formatted_file() {
    let mut server = TestServer::new().await;
    server.open("wswu_clean.php", "<?php\n").await;

    let resp = server.will_save_wait_until("wswu_clean.php").await;
    assert!(resp["error"].is_null(), "unexpected error: {resp:?}");
    let result = &resp["result"];
    assert!(
        result.is_null() || result.as_array().map(|a| a.is_empty()).unwrap_or(false),
        "expected null or empty edits for already-formatted file: {resp:?}"
    );
}

#[tokio::test]
async fn will_save_wait_until_returns_null_or_edits_for_unformatted_file() {
    let mut server = TestServer::new().await;
    server
        .open("wswu_ugly.php", "<?php\nfunction ugly( $x ){return $x;}\n")
        .await;

    let resp = server.will_save_wait_until("wswu_ugly.php").await;
    assert!(resp["error"].is_null(), "unexpected error: {resp:?}");

    let result = &resp["result"];
    if let Some(edits) = result.as_array() {
        for edit in edits {
            assert!(
                edit["range"]["start"].is_object() && edit["range"]["end"].is_object(),
                "edit missing range: {edit:?}"
            );
            assert!(
                edit["newText"].is_string(),
                "edit missing newText: {edit:?}"
            );
        }
    } else {
        assert!(result.is_null(), "expected null or array, got: {result:?}");
    }
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

    assert!(
        resp["error"].is_null(),
        "hover after change should not error"
    );
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
    assert!(
        !links.is_empty(),
        "expected at least one link for require_once path"
    );
}

#[tokio::test]
async fn inline_value_returns_array() {
    let mut server = TestServer::new().await;
    server
        .open("inlval.php", "<?php\n$x = 42;\n$y = $x + 1;\n")
        .await;

    let resp = server.inline_value("inlval.php", 2, 0, 2, 10).await;

    assert!(resp["error"].is_null(), "inlineValue error: {:?}", resp);
    let values = resp["result"]
        .as_array()
        .expect("inlineValue must return an array when variables are in range");
    assert_eq!(values.len(), 2, "expected exactly $y and $x on line 2");
    let names: Vec<&str> = values
        .iter()
        .filter_map(|v| v["variableName"].as_str())
        .collect();
    assert!(
        names.contains(&"y"),
        "expected variable 'y' ($y), got: {:?}",
        names
    );
    assert!(
        names.contains(&"x"),
        "expected variable 'x' ($x), got: {:?}",
        names
    );
}
