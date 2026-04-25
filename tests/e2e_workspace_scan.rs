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

/// A composer.json that points a PSR-4 prefix at a directory that doesn't
/// exist on disk must not crash or stall the scan — existing directories
/// must still be indexed, and features on opened files in the valid
/// directory must still work.
#[tokio::test]
async fn nonexistent_psr4_dir_does_not_crash_server() {
    let mut server = TestServer::with_fixture("missing-psr4-dir").await;
    server.wait_for_index_ready().await;

    // `Present\Alive` lives under an existing PSR-4 root and must still be
    // discoverable via workspace symbols — the missing `src/Ghost/` root must
    // have been skipped silently.
    let resp = server.workspace_symbols("Alive").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| {
            s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Present/Alive.php"))
                .unwrap_or(false)
        }),
        "Alive in existing PSR-4 root must be indexed despite sibling missing dir, got: {symbols:?}"
    );

    // Opening the file and requesting document symbols exercises the parser +
    // PSR-4 resolution path end-to-end.
    let (text, _, _) = server.locate("src/Present/Alive.php", "<?php", 0);
    server.open("src/Present/Alive.php", &text).await;
    let resp = server.document_symbols("src/Present/Alive.php").await;
    assert!(
        resp["error"].is_null(),
        "documentSymbol errored with missing PSR-4 dir in composer: {resp:?}"
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
    let mut server = TestServer::with_fixture_and_options(
        "psr4-mini",
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
    assert!(
        !symbols.iter().any(|s| {
            s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Service/Greeter.php"))
                .unwrap_or(false)
        }),
        "Greeter is in excluded src/Service — must not be indexed, got: {symbols:?}"
    );

    // User is in src/Model — it must still be indexed.
    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| {
            s["location"]["uri"]
                .as_str()
                .map(|u| u.ends_with("src/Model/User.php"))
                .unwrap_or(false)
        }),
        "User is NOT excluded — must still appear in workspace symbols, got: {symbols:?}"
    );
}

/// `excludePaths` set in `.php-lsp.json` must be honored by the workspace scan,
/// even when no `initializationOptions` are provided by the editor.
#[tokio::test]
async fn php_lsp_json_exclude_paths_honored() {
    // Copy psr4-mini into a temp dir and add .php-lsp.json before the server starts.
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("tests/fixtures/psr4-mini");
    let tmp = tempfile::tempdir().expect("create TempDir");
    // Copy the fixture.
    fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            let to = dst.join(e.file_name());
            if e.file_type()?.is_dir() {
                copy_dir(&e.path(), &to)?;
            } else {
                std::fs::copy(e.path(), to)?;
            }
        }
        Ok(())
    }
    copy_dir(&source, tmp.path()).unwrap();
    // Write .php-lsp.json that excludes src/Service/*.
    std::fs::write(
        tmp.path().join(".php-lsp.json"),
        r#"{"excludePaths": ["src/Service/*"]}"#,
    )
    .unwrap();

    let mut server = TestServer::with_root(tmp.path()).await;
    server.wait_for_index_ready().await;

    // Greeter lives in src/Service — must not appear.
    let resp = server.workspace_symbols("Greeter").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.ends_with("src/Service/Greeter.php"))
            .unwrap_or(false)),
        "Greeter is excluded via .php-lsp.json — must not be indexed, got: {symbols:?}"
    );

    // User lives in src/Model — must still appear.
    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.ends_with("src/Model/User.php"))
            .unwrap_or(false)),
        "User is not excluded — must still be indexed, got: {symbols:?}"
    );
}

/// `excludePaths` from `.php-lsp.json` and from `initializationOptions` must
/// be concatenated (not replaced): both sources of exclusions must apply.
#[tokio::test]
async fn php_lsp_json_exclude_paths_concat_with_editor() {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let source = manifest_dir.join("tests/fixtures/psr4-mini");
    let tmp = tempfile::tempdir().expect("create TempDir");
    fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            let to = dst.join(e.file_name());
            if e.file_type()?.is_dir() {
                copy_dir(&e.path(), &to)?;
            } else {
                std::fs::copy(e.path(), to)?;
            }
        }
        Ok(())
    }
    copy_dir(&source, tmp.path()).unwrap();
    // File excludes src/Service; editor excludes src/Model.
    std::fs::write(
        tmp.path().join(".php-lsp.json"),
        r#"{"excludePaths": ["src/Service/*"]}"#,
    )
    .unwrap();

    let mut server = TestServer::with_root_and_options(
        tmp.path(),
        json!({
            "diagnostics": { "enabled": true },
            "excludePaths": ["src/Model/*"],
        }),
    )
    .await;
    server.wait_for_index_ready().await;

    // Greeter (src/Service) must not appear.
    let resp = server.workspace_symbols("Greeter").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.ends_with("src/Service/Greeter.php"))
            .unwrap_or(false)),
        "Greeter excluded via .php-lsp.json, got: {symbols:?}"
    );

    // User (src/Model) must not appear either.
    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.ends_with("src/Model/User.php"))
            .unwrap_or(false)),
        "User excluded via initializationOptions, got: {symbols:?}"
    );
}
