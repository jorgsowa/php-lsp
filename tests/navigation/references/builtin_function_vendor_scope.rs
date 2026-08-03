//! Regression pin for ROADMAP item 0c / plan step 0 (`~/.claude/plans/crispy-noodling-key.md`).
//!
//! Same vendor-scoping problem as `builtin_vendor_scope.rs`, but for
//! `Name::Function` instead of `Name::Class`: `reference_candidate_files`
//! (`src/document/document_store.rs:1218-1223`) only narrows a function/
//! constant reference when its FQCN is namespaced — an unqualified call to a
//! *global* builtin (`array_map`, `strlen`, ...) is unqualified by
//! definition, so it falls straight through to the full workspace today,
//! same as `Closure`. `mir::stubs::is_builtin_function` already exists and
//! is already re-exported (used by hover's php.net links) — this needs a
//! new branch in `reference_candidate_files`, not a new mir API.
//!
//! `Name::GlobalConstant` shares the exact same match arm and has the exact
//! same problem for builtin constants (`PHP_EOL`, `PHP_VERSION`, ...), but
//! there's no test for it here: mir's equivalent helper
//! (`stub_path_for_constant`) is `pub(crate)`, not re-exported — fixing the
//! constant case needs a small mir change (make it `pub`, or add an
//! `is_builtin_constant` wrapper) before it's even reachable from php-lsp.

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

/// Global builtin function call, no import needed (PHP falls back to the
/// global function when the current namespace doesn't declare one of the
/// same name) — matches how this is written in real code. Vendor calls it
/// too; only the project call must remain in the post-fix result.
#[tokio::test]
#[ignore = "vendor scoping for builtin-resolved symbols not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_global_builtin_function_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run(array $items): array {\n        return array_map(fn($x) => $x, $items);\n    }\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Runner.php"), vendor_caller).unwrap();
    let project_caller = "<?php\nnamespace App;\n\nclass Handler {\n    public function handle(array $items): array {\n        return array_map(fn($x) => $x, $items);\n    }\n}\n".to_string();
    std::fs::write(dir.path().join("src/Handler.php"), &project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &project_caller).await;

    let (_, line, col) = server.locate("src/Handler.php", "array_map", 0);
    let resp = server.references("src/Handler.php", line, col + 1, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:5:15-5:24"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Explicit global-escape syntax (`\array_map(...)`) — a real PHP idiom
/// used from inside a namespace to be unambiguous. This exercises whether
/// the eventual builtin check strips a leading backslash before doing its
/// lookup (mir's class-side equivalent, `stub_path_for_class`, already does
/// this — `is_builtin_function`'s call site in the fix needs the same
/// treatment, since the function-name index itself is not backslash-aware).
#[tokio::test]
#[ignore = "vendor scoping for builtin-resolved symbols not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_backslash_prefixed_builtin_function_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run(array $items): array {\n        return \\array_map(fn($x) => $x, $items);\n    }\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Runner.php"), vendor_caller).unwrap();
    let project_caller = "<?php\nnamespace App;\n\nclass Handler {\n    public function handle(array $items): array {\n        return \\array_map(fn($x) => $x, $items);\n    }\n}\n".to_string();
    std::fs::write(dir.path().join("src/Handler.php"), &project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &project_caller).await;

    let (_, line, col) = server.locate("src/Handler.php", "array_map", 0);
    let resp = server.references("src/Handler.php", line, col + 1, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:5:15-5:25"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Control case: a *namespaced* function that merely shares a builtin's
/// short name must keep vendor usages in scope. Structurally this can't
/// regress from the fix above — a namespaced FQCN already returns early via
/// `fqn_reachable_files` before any builtin check would run — but it's
/// cheap insurance against a future refactor that reorders those checks.
/// NOT ignored — must hold today and after the fix lands.
#[tokio::test]
async fn references_on_namespaced_function_shadowing_builtin_name_still_includes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let decl = "<?php\nnamespace App;\n\nfunction array_map(callable $fn, array $items): array {\n    return [];\n}\n".to_string();
    std::fs::write(dir.path().join("src/functions.php"), &decl).unwrap();
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run(array $items): array {\n        return \\App\\array_map(fn($x) => $x, $items);\n    }\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Runner.php"), vendor_caller).unwrap();
    let project_caller = "<?php\nnamespace App;\n\nclass Handler {\n    public function handle(array $items): array {\n        return array_map(fn($x) => $x, $items);\n    }\n}\n";
    std::fs::write(dir.path().join("src/Handler.php"), project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/functions.php", &decl).await;

    let (_, line, col) = server.locate("src/functions.php", "function array_map", 0);
    let resp = server.references("src/functions.php", line, col + 9, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect![[r#"
        src/Handler.php:5:15-5:24
        vendor/acme/lib/src/Runner.php:5:15-5:29"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}
