//! Regression pin for ROADMAP item 0c / plan step 0 (`~/.claude/plans/crispy-noodling-key.md`).
//!
//! A third variant of the same vendor-scoping gap, this time for methods
//! *declared on* a builtin-stub-resolved class — `Closure::fromCallable()`
//! (static) and `$closure->call()` (instance). `method_reference_scope_plan`
//! (`src/document/document_store.rs`) returns `MethodScopePlan::FullWorkspace`
//! unconditionally for any public method, builtin-owned or not — narrowing
//! this needs to check the *owner*'s FQCN against `stub_path_for_class`
//! before falling through to the existing (correct, and separately tracked)
//! full-workspace behavior for vendor-defined public methods.

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

/// `Closure::fromCallable()` — a static method owned by a builtin. Cursor is
/// on `fromCallable`, not `Closure`, so this resolves as `Name::Method`, not
/// `Name::Class` (already covered by `builtin_vendor_scope.rs`).
#[tokio::test]
#[ignore = "vendor scoping for builtin-owned methods not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_closure_static_method_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run($fn): \\Closure {\n        return \\Closure::fromCallable($fn);\n    }\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Runner.php"), vendor_caller).unwrap();
    let project_caller = "<?php\nnamespace App;\n\nuse Closure;\n\nclass Handler {\n    public function handle($fn): Closure {\n        return Closure::fromCallable($fn);\n    }\n}\n".to_string();
    std::fs::write(dir.path().join("src/Handler.php"), &project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &project_caller).await;

    let (_, line, col) = server.locate("src/Handler.php", "fromCallable", 0);
    let resp = server.references("src/Handler.php", line, col + 1, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:7:24-7:36"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// `$cb->call($this)` — an instance method owned by a builtin, dispatched on
/// a typed variable rather than a class-name expression.
#[tokio::test]
#[ignore = "vendor scoping for builtin-owned methods not implemented yet (ROADMAP 0c step 0)"]
async fn references_on_closure_instance_method_excludes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Runner {\n    public function run(\\Closure $cb): mixed {\n        return $cb->call($this);\n    }\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Runner.php"), vendor_caller).unwrap();
    let project_caller = "<?php\nnamespace App;\n\nuse Closure;\n\nclass Handler {\n    public function handle(Closure $cb): mixed {\n        return $cb->call($this);\n    }\n}\n".to_string();
    std::fs::write(dir.path().join("src/Handler.php"), &project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("src/Handler.php", &project_caller).await;

    let (_, line, col) = server.locate("src/Handler.php", "->call(", 0);
    let resp = server.references("src/Handler.php", line, col + 3, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect!["src/Handler.php:7:20-7:24"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Control case: an unrelated *vendor-defined* class with its own public
/// method named `call` (same short name as `Closure::call`) must keep
/// vendor usages in scope. Guards against a naive fix keying off method
/// name alone instead of checking the owner's FQCN. NOT ignored — must hold
/// today and after the fix lands.
#[tokio::test]
async fn references_on_unrelated_class_method_named_call_still_includes_vendor_usages() {
    let dir = tempfile::tempdir().unwrap();
    write_composer(dir.path());
    let widget = "<?php\nnamespace Acme\\Lib;\n\nclass Widget {\n    public function call(): void {}\n}\n";
    std::fs::write(dir.path().join("vendor/acme/lib/src/Widget.php"), widget).unwrap();
    let vendor_caller = "<?php\nnamespace Acme\\Lib;\n\nclass Factory {\n    public function make(Widget $w): void {\n        $w->call();\n    }\n}\n";
    std::fs::write(
        dir.path().join("vendor/acme/lib/src/Factory.php"),
        vendor_caller,
    )
    .unwrap();
    let project_caller = "<?php\nnamespace App;\n\nuse Acme\\Lib\\Widget;\n\nfunction make(Widget $w): void {\n    $w->call();\n}\n";
    std::fs::write(dir.path().join("src/App.php"), project_caller).unwrap();

    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;
    server.open("vendor/acme/lib/src/Widget.php", widget).await;

    let (_, line, col) = server.locate("vendor/acme/lib/src/Widget.php", "function call", 0);
    let resp = server
        .references("vendor/acme/lib/src/Widget.php", line, col + 9, false)
        .await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");

    expect![[r#"
        src/App.php:6:8-6:12
        vendor/acme/lib/src/Factory.php:5:12-5:16"#]]
    .assert_eq(&render_locations(&resp, &server.uri("")));
}
