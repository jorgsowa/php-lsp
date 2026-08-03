//! Regression pin for ROADMAP item 0c / plan step 0 (`~/.claude/plans/crispy-noodling-key.md`).
//!
//! Same vendor-scoping rule as `builtin_function_vendor_scope.rs`, but for
//! `Name::GlobalConstant` instead of `Name::Function`: a builtin constant
//! (`PHP_EOL`, `PHP_VERSION`, ...) is never declared in vendor, so vendor's
//! own usages of it are dependency-internal noise, same as a builtin class
//! or function.
//!
//! Unlike the class/function cases, this one is `#[ignore]`d and stays that
//! way for now: narrowing it needs `mir_analyzer::is_builtin_constant`,
//! which only exists on mir's `main` as of the `is-builtin-constant` branch
//! merge (commit `51c5d53b`) — not yet released, and php-lsp's `Cargo.lock`
//! still pins the prior mir commit (`1cd6682b`, tag `0.67.0`). Un-ignore
//! these once php-lsp bumps its mir pin past that merge AND
//! `reference_candidate_files`'s `Name::GlobalConstant` arm
//! (`src/document/document_store.rs`) gets the same builtin check the
//! `Name::Function` arm already has.

use super::*;
use expect_test::expect;

fn write_composer(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("vendor/acme/lib/src")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("composer.json"),
        r#"{"autoload":{"psr-4":{"Acme\\Lib\\":"vendor/acme/lib/src/","App\\":"src/"}}}"#,
    )
    .unwrap();
}

/// Global builtin constant, no import needed (PHP falls back to the global
/// constant when the current namespace doesn't declare one of the same
/// name) — matches how this is written in real code. Vendor references it
/// too; only the project usage must remain in the result.
#[tokio::test]
#[ignore = "vendor scoping for builtin constants not implemented yet (ROADMAP 0c step 0, blocked on an unreleased mir pin bump)"]
async fn references_on_global_builtin_constant_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run(): string {\n        return 'x' . PHP_EOL;\n    }\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Runner.php"), vendor_caller).unwrap();
    let project_caller = "<?php\nnamespace App;\n\nclass Handler {\n    public function handle(): string {\n        return 'x' . PHP_EOL;\n    }\n}\n".to_string();
    std::fs::write(dir.path().join("src/Handler.php"), &project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &project_caller).await;

    let (_, line, col) = server.locate("src/Handler.php", "PHP_EOL", 0);
    let resp = server.references("src/Handler.php", line, col + 1, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:5:21-5:28"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Explicit global-escape syntax (`\PHP_EOL`) — a real PHP idiom used from
/// inside a namespace to be unambiguous. This exercises that the builtin
/// check strips a leading backslash before doing its lookup, the same
/// treatment `stub_path_for_class`/`is_builtin_function` already get.
#[tokio::test]
#[ignore = "vendor scoping for builtin constants not implemented yet (ROADMAP 0c step 0, blocked on an unreleased mir pin bump)"]
async fn references_on_backslash_prefixed_builtin_constant_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run(): string {\n        return 'x' . \\PHP_EOL;\n    }\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Runner.php"), vendor_caller).unwrap();
    let project_caller = "<?php\nnamespace App;\n\nclass Handler {\n    public function handle(): string {\n        return 'x' . \\PHP_EOL;\n    }\n}\n".to_string();
    std::fs::write(dir.path().join("src/Handler.php"), &project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &project_caller).await;

    let (_, line, col) = server.locate("src/Handler.php", "PHP_EOL", 0);
    let resp = server.references("src/Handler.php", line, col + 1, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:5:22-5:29"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Control case: a *namespaced* constant that merely shares a builtin's
/// short name must keep vendor usages in scope. Structurally this can't
/// regress from the builtin-narrowing case above — a namespaced FQCN
/// already returns early via `fqn_reachable_files` before any builtin check
/// runs — but it's cheap insurance against a future refactor that reorders
/// those checks.
#[tokio::test]
async fn references_on_namespaced_constant_shadowing_builtin_name_still_includes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let decl = "<?php\nnamespace App;\n\nconst PHP_EOL = \"\\n\";\n".to_string();
    std::fs::write(dir.path().join("src/constants.php"), &decl).unwrap();
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run(): string {\n        return 'x' . \\App\\PHP_EOL;\n    }\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Runner.php"), vendor_caller).unwrap();
    let project_caller = "<?php\nnamespace App;\n\nclass Handler {\n    public function handle(): string {\n        return 'x' . PHP_EOL;\n    }\n}\n";
    std::fs::write(dir.path().join("src/Handler.php"), project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/constants.php", &decl).await;

    let (_, line, col) = server.locate("src/constants.php", "const PHP_EOL", 0);
    let resp = server.references("src/constants.php", line, col + 6, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect![[r#"
        src/Handler.php:5:21-5:28
        vendor/acme/lib/src/Runner.php:5:26-5:33"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}
