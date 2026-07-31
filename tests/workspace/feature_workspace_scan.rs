//! Workspace scan: didChangeWatchedFiles CREATED/CHANGED/DELETED events,
//! and excludePaths filtering from initializationOptions and .php-lsp.json.

use super::*;

use expect_test::expect;
use serde_json::json;
use std::time::Duration;

const CREATED: u32 = 1;
const CHANGED: u32 = 2;
const DELETED: u32 = 3;

// ── indexReady timing ────────────────────────────────────────────────────────

/// `$/php-lsp/indexReady` must fire as soon as the scan (parse + index) phase
/// completes, not after the background salsa warmup.  On large workspaces the
/// warmup can take tens of seconds; blocking on it caused the notification to
/// never arrive within normal test timeouts.
#[tokio::test]
async fn index_ready_fires_after_scan_and_workspace_symbols_work() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    s.wait_for_index_ready().await;
    let out = s.snapshot_workspace_symbols("User").await;
    expect![[r#"
        Class       User @ src/Model/User.php:4
        Property    $users @ src/Service/Registry.php:9"#]]
    .assert_eq(&out);
}

/// The workspace scan must emit standard `$/progress` Report notifications
/// (with `kind: "report"` and a `percentage`) between Begin and End so editors
/// can render a live progress bar during multi-second cold scans.
#[tokio::test]
async fn workspace_scan_emits_work_done_progress_reports() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    let notifications = s.client().collect_until_index_ready().await;

    // At minimum one $/progress Report notification must arrive. A single-chunk
    // scan (all files fit in one chunk) still emits one report at 100% so that
    // editors always see a concrete progress update before the End notification.
    let has_report = notifications.iter().any(|n| {
        n["method"].as_str() == Some("$/progress")
            && n["params"]["value"]["kind"].as_str() == Some("report")
            && n["params"]["value"]["percentage"].as_u64().is_some()
    });
    assert!(
        has_report,
        "expected at least one $/progress notification with kind='report' and 'percentage';\n\
         got: {notifications:#?}"
    );
}

// ── CREATED ──────────────────────────────────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn created_file_becomes_discoverable_via_workspace_symbols() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.snapshot_workspace_symbols("Widget").await;
    expect![[r#"<no symbols>"#]].assert_eq(&pre);

    server.write_file(
        "src/Service/Widget.php",
        "<?php\nnamespace App\\Service;\n\nclass Widget {}\n",
    );
    let uri = server.uri("src/Service/Widget.php");
    server.did_change_watched_files(vec![(uri, CREATED)]).await;

    server
        .wait_until_symbol_present("Widget", Duration::from_secs(3))
        .await;

    let post = server.snapshot_workspace_symbols("Widget").await;
    expect![[r#"Class       Widget @ src/Service/Widget.php:3"#]].assert_eq(&post);
}

#[serial_test::serial]
#[tokio::test]
async fn created_file_in_new_subdirectory_is_indexed() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    server.write_file(
        "src/Queue/Job.php",
        "<?php\nnamespace App\\Queue;\n\nclass Job {}\n",
    );
    let uri = server.uri("src/Queue/Job.php");
    server.did_change_watched_files(vec![(uri, CREATED)]).await;

    server
        .wait_until_symbol_present("Job", Duration::from_secs(3))
        .await;

    let out = server.snapshot_workspace_symbols("Job").await;
    expect![[r#"Class       Job @ src/Queue/Job.php:3"#]].assert_eq(&out);
}

// ── CHANGED ───────────────────────────────────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn changed_file_updates_workspace_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.snapshot_workspace_symbols("Greeter").await;
    expect![[r#"Class       Greeter @ src/Service/Greeter.php:6"#]].assert_eq(&pre);

    server.write_file(
        "src/Service/Greeter.php",
        "<?php\nnamespace App\\Service;\n\nclass GreeterUpdated {}\n",
    );
    let uri = server.uri("src/Service/Greeter.php");
    server.did_change_watched_files(vec![(uri, CHANGED)]).await;

    server
        .wait_until_symbol_present("GreeterUpdated", Duration::from_secs(3))
        .await;

    let post = server.snapshot_workspace_symbols("GreeterUpdated").await;
    expect![[r#"Class       GreeterUpdated @ src/Service/Greeter.php:3"#]].assert_eq(&post);

    let gone = server.snapshot_workspace_symbols("Greeter").await;
    expect![[r#"Class       GreeterUpdated @ src/Service/Greeter.php:3"#]].assert_eq(&gone);
}

// ── DELETED ───────────────────────────────────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn deleted_file_symbols_removed_from_index() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let pre = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"Class       Registry @ src/Service/Registry.php:6"#]].assert_eq(&pre);

    server.remove_file("src/Service/Registry.php");
    let uri = server.uri("src/Service/Registry.php");
    server.did_change_watched_files(vec![(uri, DELETED)]).await;

    server
        .wait_until_symbol_absent("Registry", Duration::from_secs(3))
        .await;

    let post = server.snapshot_workspace_symbols("Registry").await;
    expect![[r#"<no symbols>"#]].assert_eq(&post);
}

// ── excludePaths ──────────────────────────────────────────────────────────────

#[serial_test::serial]
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

#[serial_test::serial]
#[tokio::test]
async fn php_lsp_json_exclude_paths_honored() {
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
    std::fs::write(
        tmp.path().join(".php-lsp.json"),
        r#"{"excludePaths": ["src/Service/*"]}"#,
    )
    .unwrap();

    let mut server = TestServer::with_root(tmp.path()).await;
    server.wait_for_index_ready().await;

    let resp = server.workspace_symbols("Greeter").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !symbols.iter().any(|s| s["location"]["uri"]
            .as_str()
            .map(|u| u.ends_with("src/Service/Greeter.php"))
            .unwrap_or(false)),
        "Greeter is excluded via .php-lsp.json — must not be indexed, got: {symbols:?}"
    );

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

// ── hidden directories ───────────────────────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn hidden_directories_are_excluded_from_scan() {
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

    // Hidden directories with PHP files must be excluded by the scan itself
    // — write them before the server starts, so `wait_for_index_ready` is
    // the only signal needed (a write after the scan has no way to reach
    // the index at all: nothing here ever sends a watched-files
    // notification, so waiting around for one is just guessing).
    let hidden_dir = tmp.path().join(".git/objects");
    std::fs::create_dir_all(&hidden_dir).unwrap();
    std::fs::write(
        hidden_dir.join("ClassInGit.php"),
        "<?php\nclass ClassInGit {}\n",
    )
    .unwrap();
    let vscode_dir = tmp.path().join(".vscode");
    std::fs::create_dir_all(&vscode_dir).unwrap();
    std::fs::write(
        vscode_dir.join("settings.php"),
        "<?php\nclass VscodeSetting {}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(tmp.path()).await;
    server.wait_for_index_ready().await;

    // Verify a known class from the fixture works
    expect!["Class       Greeter @ src/Service/Greeter.php:6"]
        .assert_eq(&server.snapshot_workspace_symbols("Greeter").await);

    // Hidden directories should NOT appear in workspace symbols
    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("ClassInGit").await);
    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("VscodeSetting").await);
}

// ── vendor directory ──────────────────────────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn vendor_directory_indexed_by_default() {
    // Eager vendor: `vendor/` is walked and declaration-indexed by default
    // (issues #240, #246), so workspace symbols cover vendor classes with no
    // extra configuration.
    let tmp = tempfile::tempdir().expect("create TempDir");
    let mut server = TestServer::with_root(tmp.path()).await;
    server.write_file(
        "vendor/symfony/http-kernel/HttpKernel.php",
        "<?php\nnamespace Symfony\\Component\\HttpKernel;\n\nclass HttpKernel {}\n",
    );

    server.wait_for_index_ready().await;

    let out = server.snapshot_workspace_symbols("HttpKernel").await;
    expect![[r#"Class       HttpKernel @ vendor/symfony/http-kernel/HttpKernel.php:3"#]]
        .assert_eq(&out);
}

#[serial_test::serial]
#[tokio::test]
async fn vendor_directory_excluded_with_index_vendor_false() {
    // Opt-out: `indexVendor: false` skips `vendor/` on the eager workspace
    // scan (useful for very large vendor trees). Vendor files still load on
    // demand via PSR-4 resolution when go-to-definition jumps into them.
    let tmp = tempfile::tempdir().expect("create TempDir");
    let mut server =
        TestServer::with_root_and_options(tmp.path(), json!({ "indexVendor": false })).await;
    server.write_file(
        "vendor/symfony/http-kernel/HttpKernel.php",
        "<?php\nnamespace Symfony\\Component\\HttpKernel;\n\nclass HttpKernel {}\n",
    );

    server.wait_for_index_ready().await;

    let out = server.snapshot_workspace_symbols("HttpKernel").await;
    expect![[r#"<no symbols>"#]].assert_eq(&out);
}

#[serial_test::serial]
#[tokio::test]
async fn vendor_directory_indexed_when_index_vendor_true() {
    // Opt-in: `indexVendor: true` restores eager-vendor behavior for users
    // who want full workspace-symbol coverage in vendor.
    let tmp = tempfile::tempdir().expect("create TempDir");
    let mut server = TestServer::with_root_and_options(
        tmp.path(),
        json!({
            "diagnostics": { "enabled": true },
            "indexVendor": true,
        }),
    )
    .await;
    server.write_file(
        "vendor/symfony/http-kernel/HttpKernel.php",
        "<?php\nnamespace Symfony\\Component\\HttpKernel;\n\nclass HttpKernel {}\n",
    );

    server.wait_for_index_ready().await;

    let out = server.snapshot_workspace_symbols("HttpKernel").await;
    expect![[r#"Class       HttpKernel @ vendor/symfony/http-kernel/HttpKernel.php:3"#]]
        .assert_eq(&out);
}

#[serial_test::serial]
#[tokio::test]
async fn vendor_directory_excluded_when_configured() {
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

    let mut server = TestServer::with_fixture_and_options(
        "psr4-mini",
        json!({
            "diagnostics": { "enabled": true },
            "excludePaths": ["vendor/"],
        }),
    )
    .await;
    server.write_file(
        "vendor/symfony/http-kernel/HttpKernel.php",
        "<?php\nnamespace Symfony\\Component\\HttpKernel;\n\nclass HttpKernel {}\n",
    );

    server.wait_for_index_ready().await;

    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("HttpKernel").await);
}

// ── file cap ──────────────────────────────────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn max_indexed_files_cap_is_enforced() {
    let tmp = tempfile::tempdir().expect("create TempDir");

    // Initialize server with custom low maxIndexedFiles limit
    let mut server = TestServer::with_root_and_options(
        tmp.path(),
        json!({
            "diagnostics": { "enabled": true },
            "maxIndexedFiles": 5,
        }),
    )
    .await;

    // Create files up to and beyond the limit
    for i in 0..10 {
        server.write_file(
            &format!("Class{}.php", i),
            &format!("<?php\nclass Class{} {{}}\n", i),
        );
    }

    server.wait_for_index_ready().await;

    // Count how many classes were indexed
    let mut indexed_count = 0;
    for i in 0..10 {
        let resp = server.workspace_symbols(&format!("Class{}", i)).await;
        if !resp["result"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
        {
            indexed_count += 1;
        }
    }

    // Must index exactly the cap: not fewer (cap broken/too aggressive) and
    // not more (cap not enforced).
    assert_eq!(
        indexed_count, 5,
        "expected exactly 5 indexed files with maxIndexedFiles=5, got {}",
        indexed_count
    );
}

#[serial_test::serial]
#[tokio::test]
async fn custom_max_indexed_files_via_init_options() {
    let tmp = tempfile::tempdir().expect("create TempDir");

    let mut server = TestServer::with_root_and_options(
        tmp.path(),
        json!({
            "diagnostics": { "enabled": true },
            "maxIndexedFiles": 3,
        }),
    )
    .await;

    // Create files beyond the limit
    for i in 0..5 {
        server.write_file(
            &format!("src/Class{}.php", i),
            &format!("<?php\nclass Class{} {{}}\n", i),
        );
    }

    server.wait_for_index_ready().await;

    // Count indexed symbols
    let mut indexed = Vec::new();
    for i in 0..5 {
        let resp = server.workspace_symbols(&format!("Class{}", i)).await;
        if !resp["result"]
            .as_array()
            .map(|a| a.is_empty())
            .unwrap_or(true)
        {
            indexed.push(i);
        }
    }

    // Must index exactly the custom cap: not fewer (cap too aggressive) and
    // not more (cap not enforced).
    assert_eq!(
        indexed.len(),
        3,
        "expected exactly 3 indexed files with maxIndexedFiles=3, got {} (files: {:?})",
        indexed.len(),
        indexed
    );
}

// ── project structure variations ──────────────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn deeply_nested_directory_structure_is_indexed() {
    let tmp = tempfile::tempdir().expect("create TempDir");
    let mut server = TestServer::with_root(tmp.path()).await;

    // Create a deeply nested structure
    server.write_file(
        "src/Level1/Level2/Level3/Level4/Level5/DeepClass.php",
        "<?php\nnamespace App\\Level1\\Level2\\Level3\\Level4\\Level5;\n\nclass DeepClass {}\n",
    );
    server.write_file(
        "src/Shallow/ShallowClass.php",
        "<?php\nnamespace App\\Shallow;\n\nclass ShallowClass {}\n",
    );

    server.wait_for_index_ready().await;

    expect!["Class       DeepClass @ src/Level1/Level2/Level3/Level4/Level5/DeepClass.php:3"]
        .assert_eq(&server.snapshot_workspace_symbols("DeepClass").await);
    expect!["Class       ShallowClass @ src/Shallow/ShallowClass.php:3"]
        .assert_eq(&server.snapshot_workspace_symbols("ShallowClass").await);
}

#[serial_test::serial]
#[tokio::test]
async fn multiple_top_level_directories_with_different_patterns() {
    let tmp = tempfile::tempdir().expect("create TempDir");
    // Opt into eager vendor indexing so the nested `packages/api/vendor` is
    // walked. Without this, lazy-vendor default skips any `vendor/` component
    // regardless of depth.
    let mut server = TestServer::with_root_and_options(
        tmp.path(),
        json!({
            "diagnostics": { "enabled": true },
            "indexVendor": true,
        }),
    )
    .await;

    // Create a multi-directory structure typical of a monorepo
    server.write_file(
        "packages/api/src/ApiService.php",
        "<?php\nnamespace App\\Api;\n\nclass ApiService {}\n",
    );
    server.write_file(
        "packages/web/src/WebService.php",
        "<?php\nnamespace App\\Web;\n\nclass WebService {}\n",
    );
    server.write_file(
        "packages/shared/src/SharedUtil.php",
        "<?php\nnamespace App\\Shared;\n\nclass SharedUtil {}\n",
    );
    server.write_file(
        "packages/api/vendor/external/External.php",
        "<?php\nclass External {}\n",
    );

    server.wait_for_index_ready().await;

    // All workspace packages should be indexed
    expect!["Class       ApiService @ packages/api/src/ApiService.php:3"]
        .assert_eq(&server.snapshot_workspace_symbols("ApiService").await);
    expect!["Class       WebService @ packages/web/src/WebService.php:3"]
        .assert_eq(&server.snapshot_workspace_symbols("WebService").await);
    expect!["Class       SharedUtil @ packages/shared/src/SharedUtil.php:3"]
        .assert_eq(&server.snapshot_workspace_symbols("SharedUtil").await);
    // Vendor in subdirectory should also be indexed (indexVendor: true)
    expect!["Class       External @ packages/api/vendor/external/External.php:1"]
        .assert_eq(&server.snapshot_workspace_symbols("External").await);
}

#[tokio::test]
async fn exclude_specific_package_in_monorepo_structure() {
    let mut server = TestServer::with_fixture_and_options(
        "psr4-mini",
        json!({
            "diagnostics": { "enabled": true },
            "excludePaths": ["vendor/", "src/Service/*"],
        }),
    )
    .await;
    server.wait_for_index_ready().await;

    // Excluded classes from src/Service/ should NOT be indexed
    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("Greeter").await);

    // But classes in src/Model/ SHOULD be indexed
    expect!["Class       User @ src/Model/User.php:4"]
        .assert_eq(&server.snapshot_workspace_symbols("User").await);
}

// ── pattern matching edge cases ───────────────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn exclude_paths_substring_match_edge_case() {
    let tmp = tempfile::tempdir().expect("create TempDir");
    let mut server = TestServer::with_root_and_options(
        tmp.path(),
        json!({
            "diagnostics": { "enabled": true },
            "excludePaths": ["src/"],
        }),
    )
    .await;

    // Files in src/ should be excluded
    server.write_file("src/ClassInSrc.php", "<?php\nclass ClassInSrc {}\n");

    // Files in directories whose name CONTAINS "src" should still be indexed
    // e.g., "tests/source/" contains "src" as substring, but pattern is "src/"
    server.write_file(
        "tests/source/ClassInTestSource.php",
        "<?php\nclass ClassInTestSource {}\n",
    );

    server.wait_for_index_ready().await;

    // ClassInSrc should be excluded (path contains "src/")
    let resp = server.workspace_symbols("ClassInSrc").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !symbols.iter().any(|s| {
            s["name"]
                .as_str()
                .map(|n| n == "ClassInSrc")
                .unwrap_or(false)
        }),
        "ClassInSrc in src/ should be excluded"
    );

    // ClassInTestSource should be indexed because pattern is "src/" not just "src"
    let resp = server.workspace_symbols("ClassInTestSource").await;
    let symbols = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        symbols.iter().any(|s| {
            s["name"]
                .as_str()
                .map(|n| n == "ClassInTestSource")
                .unwrap_or(false)
        }),
        "ClassInTestSource in tests/source/ should be indexed (pattern is 'src/', not 'src')"
    );
}

#[serial_test::serial]
#[tokio::test]
async fn exclude_paths_does_not_substring_match_intermediate_dirs() {
    let tmp = tempfile::tempdir().expect("create TempDir");
    let mut server = TestServer::with_root_and_options(
        tmp.path(),
        json!({
            "diagnostics": { "enabled": true },
            "excludePaths": ["src/"],
        }),
    )
    .await;

    // File in "test_src/" should NOT be excluded even though it contains "src"
    server.write_file(
        "test_src/ClassInTestSrc.php",
        "<?php\nclass ClassInTestSrc {}\n",
    );

    server.wait_for_index_ready().await;

    // ClassInTestSrc should be indexed because pattern "src/" doesn't match "test_src/"
    expect!["Class       ClassInTestSrc @ test_src/ClassInTestSrc.php:1"]
        .assert_eq(&server.snapshot_workspace_symbols("ClassInTestSrc").await);
}

#[tokio::test]
async fn empty_workspace_returns_zero_files() {
    let tmp = tempfile::tempdir().expect("create TempDir");
    let mut server = TestServer::with_root(tmp.path()).await;

    server.wait_for_index_ready().await;

    // Querying an empty workspace should return no symbols
    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("NonExistent").await);
}

#[serial_test::serial]
#[tokio::test]
async fn exclude_all_paths_leaves_nothing_indexed() {
    let mut server = TestServer::with_fixture_and_options(
        "psr4-mini",
        json!({
            "diagnostics": { "enabled": true },
            "excludePaths": ["src/", "vendor/"],
        }),
    )
    .await;

    server.wait_for_index_ready().await;

    // When excluding all real code (src and vendor), even the fixture classes should be missing
    // (Though psr4-mini fixture has code in src/, so it will all be excluded)
    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("Greeter").await);
}

// ── includePaths (override excludePaths) ──────────────────────────────────────

#[serial_test::serial]
#[tokio::test]
async fn include_paths_override_exclude_paths() {
    // Exclude vendor/* but explicitly include vendor/yiisoft.
    let mut server = TestServer::with_fixture_and_options(
        "yii-demo",
        json!({
            "diagnostics": { "enabled": false },
            "excludePaths": ["vendor/*"],
            "includePaths": ["vendor/yiisoft"],
        }),
    )
    .await;
    server.wait_for_index_ready().await;

    // Files under vendor/noyiisoft should be excluded...
    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("Fish").await);

    // ...except the explicitly included subdirectory.
    expect!["Class       Translator @ vendor/yiisoft/Translator.php:4"]
        .assert_eq(&server.snapshot_workspace_symbols("Translator").await);
}

#[serial_test::serial]
#[tokio::test]
async fn include_paths_only_affects_matched_entries() {
    // Exclude src/Service/* but include only src/Service/Greeter.php.
    let mut server = TestServer::with_fixture_and_options(
        "psr4-mini",
        json!({
            "diagnostics": { "enabled": false },
            "excludePaths": ["src/Service/*"],
            "includePaths": ["Greeter"],
        }),
    )
    .await;
    server.wait_for_index_ready().await;

    // Greeter should be indexed (included).
    expect!["Class       Greeter @ src/Service/Greeter.php:6"]
        .assert_eq(&server.snapshot_workspace_symbols("Greeter").await);

    // Registry (also in src/Service/) should NOT be indexed.
    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("Registry").await);

    // User (in src/Model/) should still be indexed.
    expect!["Class       User @ src/Model/User.php:4"]
        .assert_eq(&server.snapshot_workspace_symbols("User").await);
}

#[serial_test::serial]
#[tokio::test]
async fn include_paths_from_php_lsp_json() {
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
    std::fs::write(
        tmp.path().join(".php-lsp.json"),
        r#"{"excludePaths": ["src/Service/*"], "includePaths": ["Greeter"]}"#,
    )
    .unwrap();

    let mut server = TestServer::with_root(tmp.path()).await;
    server.wait_for_index_ready().await;

    // Greeter included via includePaths.
    expect!["Class       Greeter @ src/Service/Greeter.php:6"]
        .assert_eq(&server.snapshot_workspace_symbols("Greeter").await);

    // Registry excluded.
    expect!["<no symbols>"].assert_eq(&server.snapshot_workspace_symbols("Registry").await);
}

// ── Chunked scan completeness ─────────────────────────────────────────────────

/// Verify that every file in a multi-file workspace appears in the workspace
/// index after `indexReady`.  The scan pipeline processes files in 500-file
/// chunks; this test guards against the last partial chunk being silently
/// dropped so that all declared classes remain discoverable.
#[serial_test::serial]
#[tokio::test]
async fn all_scanned_files_appear_in_workspace_index_after_index_ready() {
    let tmp = tempfile::tempdir().expect("create TempDir");
    let mut server = TestServer::with_root(tmp.path()).await;

    let classes = [
        ("src/Alpha.php", "Alpha"),
        ("src/Beta.php", "Beta"),
        ("src/Gamma.php", "Gamma"),
        ("src/Sub/Delta.php", "Delta"),
        ("src/Sub/Epsilon.php", "Epsilon"),
    ];

    for (path, name) in &classes {
        server.write_file(
            path,
            &format!("<?php\nnamespace App;\n\nclass {name} {{}}\n"),
        );
    }

    server.wait_for_index_ready().await;

    let mut results = Vec::new();
    for (_, name) in &classes {
        results.push(server.snapshot_workspace_symbols(name).await);
    }
    expect![[r#"
        Class       Alpha @ src/Alpha.php:3
        Class       Beta @ src/Beta.php:3
        Class       Gamma @ src/Gamma.php:3
        Class       Delta @ src/Sub/Delta.php:3
        Class       Epsilon @ src/Sub/Epsilon.php:3"#]]
    .assert_eq(&results.join("\n"));
}

// ── workspace/symbol substring matching ──────────────────────────────────────

/// A query that is a substring (not a prefix) of a class name must still return
/// a match.  Previously `fuzzy_camel_match` only matched prefixes and camelCase
/// abbreviations, so "reeter" never matched "Greeter" and "Controller" never
/// matched "BlogController".
#[tokio::test]
async fn workspace_symbols_substring_match() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    s.wait_for_index_ready().await;
    // "reeter" is a suffix of "Greeter" — must match via substring fallback
    let out = s.snapshot_workspace_symbols("reeter").await;
    expect![[r#"Class       Greeter @ src/Service/Greeter.php:6"#]].assert_eq(&out);
}

// Note: `include_paths_concatenated_with_editor_config` was removed because
// it relied on `change_configuration`, which triggers a server→client
// `workspace/configuration` request that the test harness does not handle
// correctly (the server calls `self.client.configuration()` internally,
// bypassing the mock).  The concatenation behavior is still covered by
// `include_paths_from_php_lsp_json` + editor init options.

/// Regression: a single unreadable (non-UTF-8) file under the workspace must
/// NOT truncate indexing. The Phase 2a read-collection loop previously used
/// `while let Some(Ok(Some(_))) = read_set.join_next()`, which stopped at the
/// first failed read — so one binary/non-UTF-8 file (common in `vendor/`)
/// dropped every file whose read completed after it.
///
/// Determinism: a 3-byte non-UTF-8 file reads in a single syscall and so always
/// finishes before the multi-megabyte valid file. With the bug, the bad file's
/// completion stops the loop before the big file is collected, so `BigKlass`
/// disappears from the index; with the fix it is always present.
#[serial_test::serial]
#[tokio::test]
async fn unreadable_file_does_not_truncate_workspace_index() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("src");
    std::fs::create_dir_all(&dir).unwrap();

    // Tiny non-UTF-8 ".php" file: read fails fast and wins the completion race.
    std::fs::write(dir.join("Binary.php"), [0xFF, 0xFE, 0x00]).unwrap();

    // A large valid file whose read reliably completes AFTER the tiny bad file.
    let big_padding = "// ".to_string() + &"x".repeat(8 * 1024 * 1024);
    std::fs::write(
        dir.join("BigKlass.php"),
        format!("<?php\nnamespace App;\n{big_padding}\nclass BigKlass {{}}\n"),
    )
    .unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;

    expect!["Class       BigKlass @ src/BigKlass.php:3"]
        .assert_eq(&s.snapshot_workspace_symbols("BigKlass").await);
}
