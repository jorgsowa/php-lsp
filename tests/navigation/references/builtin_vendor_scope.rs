//! Regression pin for ROADMAP item 0c / plan step 0 (`~/.claude/plans/crispy-noodling-key.md`).
//!
//! `Closure`, `ReflectionParameter`, and other PHP core/extension types are
//! declared via mir's bundled stub set (`mir::stubs::stub_path_for_class`),
//! never in `vendor/` — a vendor file that type-hints `Closure` is the PHP
//! equivalent of a dependency's compiled JS mentioning `Promise`: not
//! something any mainstream tool treats as a project reference. Contrast
//! with a *vendor-defined* class/interface, where a vendor-internal usage is
//! genuinely informative (cross-package integration) and must stay in scope.
//!
//! `reference_candidate_files` (`src/document/document_store.rs`) does not
//! yet special-case builtin-stub-resolved symbols, so today's candidate scope
//! for `Closure` includes vendor. These tests are `#[ignore]`d until that
//! scoping lands; un-ignore them as part of implementing plan step 0.

use super::*;
use expect_test::expect;

// `namespace Acme\Lib;` resolves a bare `Closure` typehint as `Acme\Lib\Closure`
// (PHP has no global-namespace fallback for class names), so the builtin must
// be referenced via a fully-qualified `\Closure` here to be a real usage.
const VENDOR_RUNNER: &str = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run(\\Closure $cb): void {\n        $cb();\n    }\n}\n";

fn project_handler() -> String {
    "<?php\nnamespace App;\n\nuse Closure;\n\nclass Handler {\n    public function handle(Closure $cb): void {\n        $cb();\n    }\n}\n".to_string()
}

fn write_vendor_fixture(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("vendor/acme/lib/src")).unwrap();
    std::fs::write(
        dir.join("composer.json"),
        r#"{"autoload":{"psr-4":{"Acme\\Lib\\":"vendor/acme/lib/src/","App\\":"src/"}}}"#,
    )
    .unwrap();
    std::fs::write(dir.join("vendor/acme/lib/src/Runner.php"), VENDOR_RUNNER).unwrap();
}

/// Cursor on the `use Closure;` import — the exact trigger from the original
/// bug report (`findReferences` on a builtin type via its import). Every
/// project-file usage of `Closure` must be found; the vendor file's own
/// `Closure` type-hint (`Runner::run`) must not.
#[tokio::test]
#[ignore = "vendor scoping for builtin-resolved symbols not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_closure_import_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_vendor_fixture(dir.path());
    let handler = project_handler();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/Handler.php"), &handler).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &handler).await;

    let (_, line, col) = server.locate("src/Handler.php", "use Closure", 0);
    // Cursor on the `C` of `Closure` (after "use ").
    let resp = server.references("src/Handler.php", line, col + 4, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    // Today (before plan step 0), this also includes
    // `vendor/acme/lib/src/Runner.php:4:24-4:32` — the vendor file's own
    // `\Closure` type-hint. This snapshot is the target post-fix state.
    expect!["src/Handler.php:6:27-6:34"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Same symbol, cursor on an ordinary usage site (the parameter type-hint)
/// rather than the import — `textDocument/references` resolves the symbol
/// under the cursor and returns every reference to it, so the invocation
/// site must not change the result. Vendor stays excluded either way.
#[tokio::test]
#[ignore = "vendor scoping for builtin-resolved symbols not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_closure_typehint_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_vendor_fixture(dir.path());
    let handler = project_handler();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/Handler.php"), &handler).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &handler).await;

    let (_, line, col) = server.locate("src/Handler.php", "Closure $cb", 0);
    let resp = server.references("src/Handler.php", line, col + 1, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    // Same target result as the import-cursor test above — invocation site
    // must not change the (post-fix) result.
    expect!["src/Handler.php:6:27-6:34"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Same symbol, cursor on a *return-type* position — the fix is resolution-
/// based (it acts on the already-resolved `Name::Class`, not the cursor's
/// syntactic context), so this must behave identically to the import/param
/// cases without any extra code. This test exists to verify that claim
/// rather than assume it.
#[tokio::test]
#[ignore = "vendor scoping for builtin-resolved symbols not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_closure_return_type_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_vendor_fixture(dir.path());
    let handler = "<?php\nnamespace App;\n\nuse Closure;\n\nclass Handler {\n    public function makeCallback(): Closure {\n        return fn() => null;\n    }\n}\n".to_string();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/Handler.php"), &handler).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &handler).await;

    let (_, line, col) = server.locate("src/Handler.php", "): Closure", 0);
    let resp = server.references("src/Handler.php", line, col + 3, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:6:36-6:43"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Typed *property* declaration — a third position beyond param/return that
/// the fix must cover for free via the same resolution-based mechanism.
#[tokio::test]
#[ignore = "vendor scoping for builtin-resolved symbols not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_closure_typed_property_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_vendor_fixture(dir.path());
    let handler = "<?php\nnamespace App;\n\nuse Closure;\n\nclass Handler {\n    private Closure $cb;\n}\n".to_string();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/Handler.php"), &handler).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &handler).await;

    let (_, line, col) = server.locate("src/Handler.php", "private Closure", 0);
    let resp = server.references("src/Handler.php", line, col + 9, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:6:12-6:19"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Nullable union form (`?Closure`) — guards against the fix accidentally
/// depending on the type-hint being a bare, unwrapped class name. If the
/// parser only extracts a `Name::Class` reference from unwrapped type-hints,
/// this would (wrongly) fall through to a different code path than the
/// plain-typehint test and could silently keep including vendor.
#[tokio::test]
#[ignore = "vendor scoping for builtin-resolved symbols not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_nullable_closure_param_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_vendor_fixture(dir.path());
    let handler = "<?php\nnamespace App;\n\nuse Closure;\n\nclass Handler {\n    public function handle(?Closure $cb): void {\n        $cb?->__invoke();\n    }\n}\n".to_string();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/Handler.php"), &handler).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &handler).await;

    let (_, line, col) = server.locate("src/Handler.php", "?Closure $cb", 0);
    let resp = server.references("src/Handler.php", line, col + 2, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:6:28-6:35"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Critical false-positive guard: a *namespaced* class that merely shares
/// its short name with a builtin (no `use Closure;`, so this file's bare
/// `Closure` typehint resolves to `Acme\Lib\Closure`, a real user-defined
/// class, per PHP's no-fallback rule for class names) must NOT be treated as
/// builtin-resolved. If the fix ever matched on short name instead of the
/// fully-qualified name, this would wrongly exclude vendor usages of a real,
/// unrelated, vendor-defined class. NOT ignored — must hold today and after
/// the fix lands.
#[tokio::test]
async fn references_on_namespaced_class_shadowing_builtin_name_still_includes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("vendor/acme/lib/src")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"Acme\\Lib\\":"vendor/acme/lib/src/","App\\":"src/"}}}"#,
    )
    .unwrap();
    // `Acme\Lib\Closure` — a real, distinct, user-defined class. Not the
    // builtin (no `use Closure;`, no leading `\`).
    let shadow = "<?php\nnamespace Acme\\Lib;\n\nclass Closure {}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Closure.php"), shadow).unwrap();
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Factory {\n    public function make(): Closure {\n        return new Closure();\n    }\n}\n";
    std::fs::write(
        dir.path().join("vendor/acme/lib/src/Factory.php"),
        vendor_caller,
    )
    .unwrap();
    let project_caller = "<?php\nnamespace App;\n\nuse Acme\\Lib\\Closure;\n\nfunction make(): Closure {\n    return new Closure();\n}\n";
    std::fs::write(dir.path().join("src/App.php"), project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("vendor/acme/lib/src/Closure.php", shadow).await;

    let (_, line, col) = server.locate("vendor/acme/lib/src/Closure.php", "class Closure", 0);
    let resp = server
        .references("vendor/acme/lib/src/Closure.php", line, col + 6, false)
        .await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect![[r#"
        src/App.php:5:17-5:24
        src/App.php:6:15-6:22
        vendor/acme/lib/src/Factory.php:4:28-4:35
        vendor/acme/lib/src/Factory.php:5:19-5:26"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Control case: a *vendor-defined* class must keep vendor usages in scope —
/// this scoping rule is specific to builtin-stub-resolved symbols, not a
/// blanket "exclude vendor" change. If this test ever needs updating because
/// vendor got excluded generally, that's a sign the implementation over-
/// broadened the rule past what plan step 0 asks for.
#[tokio::test]
async fn references_on_vendor_defined_class_still_includes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("vendor/acme/lib/src")).unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"Acme\\Lib\\":"vendor/acme/lib/src/","App\\":"src/"}}}"#,
    )
    .unwrap();
    let widget = "<?php\nnamespace Acme\\Lib;\n\nclass Widget {}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Widget.php"), widget).unwrap();
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Factory {\n    public function make(): Widget {\n        return new Widget();\n    }\n}\n";
    std::fs::write(
        dir.path().join("vendor/acme/lib/src/Factory.php"),
        vendor_caller,
    )
    .unwrap();
    let project_caller = "<?php\nnamespace App;\n\nuse Acme\\Lib\\Widget;\n\nfunction make(): Widget {\n    return new Widget();\n}\n";
    std::fs::write(dir.path().join("src/App.php"), project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("vendor/acme/lib/src/Widget.php", widget).await;

    let (_, line, col) = server.locate("vendor/acme/lib/src/Widget.php", "class Widget", 0);
    let resp = server
        .references("vendor/acme/lib/src/Widget.php", line, col + 6, false)
        .await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect![[r#"
        src/App.php:5:17-5:23
        src/App.php:6:15-6:21
        vendor/acme/lib/src/Factory.php:4:28-4:34
        vendor/acme/lib/src/Factory.php:5:19-5:25"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}
