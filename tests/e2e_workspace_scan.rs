//! Edge cases in the workspace-scan path.
//!
//! These are the scenarios most likely to panic or hang the server on a
//! cold start: no composer.json, malformed composer.json, excludePaths
//! that the scan must honor. rust-analyzer's equivalent is
//! `project_model::tests` — this file is the wire-protocol analogue.

mod common;

use common::TestServer;
use serde_json::json;

/// A workspace with no composer.json must initialize cleanly: the
/// workspace-scan indexReady notification arrives, and intra-file features
/// still work on opened files (hover, definition).
#[tokio::test]
async fn workspace_without_composer_json_still_works() {
    let mut server = TestServer::with_fixture("no-composer").await;
    server.wait_for_index_ready().await;

    // Hover on the declaration should still surface the function signature.
    let (text, line, ch) = server.locate("src/standalone.php", "standalone", 0);
    server.open("src/standalone.php", &text).await;
    let resp = server.hover("src/standalone.php", line, ch).await;
    assert!(resp["error"].is_null(), "hover errored: {resp:?}");
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("standalone") && contents.contains("int"),
        "hover must still work without composer.json, got: {contents}"
    );
}

/// A malformed composer.json must not crash the server or block the scan —
/// the server must still accept requests on the workspace's files.
#[tokio::test]
async fn malformed_composer_json_does_not_crash_server() {
    let mut server = TestServer::with_fixture("broken-composer").await;
    server.wait_for_index_ready().await;

    let (text, _, _) = server.locate("src/Thing.php", "<?php", 0);
    server.open("src/Thing.php", &text).await;

    // Any request on the opened file should succeed — document symbols is
    // a good smoke signal since it exercises the parser + index path.
    let resp = server.document_symbols("src/Thing.php").await;
    assert!(
        resp["error"].is_null(),
        "documentSymbol errored after malformed composer: {resp:?}"
    );
    let result = &resp["result"];
    let has_thing = result
        .as_array()
        .map(|arr| {
            arr.iter().any(|s| {
                s["name"].as_str() == Some("Thing") || s["name"].as_str() == Some("App\\Thing")
            })
        })
        .unwrap_or(false);
    assert!(
        has_thing,
        "expected `Thing` in document symbols despite broken composer, got: {result:?}"
    );
}

/// Files matching a path in `excludePaths` (via `initializationOptions`)
/// must not be indexed — a workspace-symbol query for a symbol defined only
/// in an excluded file must not find it.
#[tokio::test]
async fn exclude_paths_honored_by_workspace_scan() {
    // Reuse the psr4-mini fixture layout but tell the server to exclude the
    // entire `src/Service` directory.
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("tests/fixtures/psr4-mini");
    let tmp = tempfile::tempdir().expect("tempdir");
    // Copy fixture manually so we own the TempDir.
    fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            let to = dst.join(e.file_name());
            if e.file_type()?.is_dir() {
                copy_dir(&e.path(), &to)?;
            } else {
                std::fs::copy(e.path(), &to)?;
            }
        }
        Ok(())
    }
    copy_dir(&source, tmp.path()).expect("copy");

    let mut server = TestServer::with_root_and_options(
        tmp.path(),
        json!({
            "diagnostics": { "enabled": true },
            "excludePaths": ["src/Service/*"],
        }),
    )
    .await;
    server.wait_for_index_ready().await;

    // Greeter is in src/Service — it must NOT appear in workspace symbols.
    let resp = server.workspace_symbols("Greeter").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    let found_greeter = symbols.iter().any(|s| {
        s["name"].as_str() == Some("Greeter")
            && s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Service/Greeter.php"))
                .unwrap_or(false)
    });
    assert!(
        !found_greeter,
        "Greeter is in excluded src/Service — must not be indexed, got: {symbols:?}"
    );

    // User is in src/Model — it must still be indexed.
    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    let found_user = symbols.iter().any(|s| {
        s["name"].as_str() == Some("User")
            && s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Model/User.php"))
                .unwrap_or(false)
    });
    assert!(
        found_user,
        "User is NOT excluded — must still appear in workspace symbols, got: {symbols:?}"
    );

    // Keep tempdir alive past server use.
    drop(server);
    drop(tmp);
}
