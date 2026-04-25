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

    // The handler publishes an empty array even for unknown files.
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

    // If a PHP formatter is installed the result is a non-empty array of TextEdits.
    // If no formatter is available the handler returns null — both are valid.
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
