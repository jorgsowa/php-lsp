//! Diagnostic coverage matrix using the caret annotation DSL.
//! Each test names the expectation inline with `// ^^^ severity: message`.

use super::*;

use expect_test::expect;
use serde_json::json;

#[tokio::test]
async fn diagnostics_published_on_did_change_for_undefined_function() {
    let mut server = TestServer::new().await;
    server.open("change_test.php", "<?php\n").await;

    let notif = server
        .change("change_test.php", 2, "<?php\nnonexistent_function();\n")
        .await;
    expect!["1:0-1:22 [1] UndefinedFunction: Function nonexistent_function() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

#[tokio::test]
async fn new_expr_fully_qualified_not_flagged_as_undefined_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Entity.php"),
        "<?php\nnamespace App\\Model;\nclass Entity {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nfunction handle(): void { $e = new \\App\\Model\\Entity(); }\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

/// Positive control: a genuinely unknown class in a `new` expression must still
/// emit UndefinedClass so the above no-false-positive tests are meaningful.
#[tokio::test]
async fn new_expr_truly_unknown_class_is_flagged() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function _wrap(): void {
    $x = new TrulyNonExistentClass9z();
//           ^^^^^^^^^^^^^^^^^^^^^^^ error: TrulyNonExistentClass9z
}
"#,
        )
        .await;
}

#[tokio::test]
async fn new_expr_with_explicit_use_alias_not_flagged_as_undefined_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Entity.php"),
        "<?php\nnamespace App\\Model;\nclass Entity {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity as EntityAlias;\nfunction handle(): void { $e = new EntityAlias(); }\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn new_expr_with_grouped_use_import_not_flagged_as_undefined_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Foo.php"),
        "<?php\nnamespace App\\Model;\nclass Foo {}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Bar.php"),
        "<?php\nnamespace App\\Model;\nclass Bar {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Model\\{Foo, Bar};\nfunction handle(): void { $a = new Foo(); $b = new Bar(); }\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn new_expr_with_use_import_not_flagged_as_undefined_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Entity.php"),
        "<?php\nnamespace App\\Model;\nclass Entity {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity;\nfunction handle(): void { $e = new Entity(); }\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

/// `use A\B\C as Alias; new Alias()` must not emit UndefinedClass.
#[tokio::test]
async fn psr4_imported_class_not_flagged_before_workspace_scan() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    // Dependency: exists on disk; lazy-loading must find it via PSR-4.
    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Entity.php"),
        "<?php\nnamespace App\\Model;\nclass Entity {}\n",
    )
    .unwrap();

    // Consuming file: uses Entity as a parameter type — the analyzer resolves
    // parameter types through use statements, exercising the full lazy-load path.
    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let handler_src = "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity;\nfunction handle(Entity $e): Entity { return $e; }\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), handler_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", handler_src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

/// Issue #243 full-stack repro: default `indexVendor: false`, a PSR-0-mapped
/// vendor class (Composer PEAR-style autoload, e.g. Magento 1 / ZF1), waiting
/// for `$/php-lsp/indexReady` before opening the consuming file.
#[tokio::test]
async fn psr0_vendor_class_not_flagged_before_workspace_scan() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-0":{"Legacy_":"vendor/legacy/src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("vendor/legacy/src/Legacy")).unwrap();
    std::fs::write(
        tmp.path().join("vendor/legacy/src/Legacy/Service.php"),
        "<?php\n\nclass Legacy_Service\n{\n    public function name(): string\n    {\n        return 'legacy';\n    }\n}\n",
    )
    .unwrap();

    let repro_src = "<?php\n\n$service = new Legacy_Service();\necho $service->name();\n";
    std::fs::write(tmp.path().join("repro.php"), repro_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("repro.php", repro_src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        repro.php
          <clean>"#]]
    .assert_eq(&out);
}

/// Fills `root` with `n` trivial decoy PHP files so the initial workspace
/// scan takes long enough to reliably lose a race against a `did_open`/pull
/// issued right after `initialized` — without this, a 1-2 file tmpdir scan
/// can finish before the very first request in either ordering depending on
/// scheduler luck, making "opened before ready" tests flaky.
fn write_decoy_files(root: &std::path::Path, n: usize) {
    for i in 0..n {
        std::fs::write(
            root.join(format!("Decoy{i}.php")),
            format!("<?php\nclass Decoy{i} {{ public function noop(): void {{}} }}\n"),
        )
        .unwrap();
    }
}

/// Issue #242: a truly undefined function in a rooted workspace must be
/// suppressed on the pre-ready `did_open` publish (the server can't yet
/// tell "not indexed" from "doesn't exist" — see
/// `compute_open_file_diagnostics`), but a corrected `publishDiagnostics`
/// must follow once the index is ready, so push-only clients aren't stuck
/// with a stale "no errors" view for the rest of the session.
#[tokio::test]
async fn open_file_diagnostics_republish_once_index_ready() {
    let tmp = tempfile::tempdir().unwrap();
    write_decoy_files(tmp.path(), 1000);
    let src = "<?php\n\ntruly_nonexistent_fn();\n";
    std::fs::write(tmp.path().join("app.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    let uri = s.uri("app.php");
    let first = s.open("app.php", src).await;
    expect!["<empty>"].assert_eq(&render_diagnostics_notification(&first));

    s.wait_for_index_ready_secs(30).await;
    let corrected = s.client().wait_for_diagnostics(&uri).await;
    expect!["2:0-2:22 [1] UndefinedFunction: Function truly_nonexistent_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&corrected));
}

/// Pull-model counterpart: `textDocument/diagnostic` must also suppress
/// workspace-resolution diagnostics before the index is ready, not just the
/// push (`did_open`) path.
#[tokio::test]
async fn pull_diagnostic_suppressed_before_index_ready() {
    let tmp = tempfile::tempdir().unwrap();
    write_decoy_files(tmp.path(), 1000);
    let src = "<?php\n\nnew TrulyMissingClass();\n";
    std::fs::write(tmp.path().join("app.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("app.php", src).await;

    let resp = s.pull_diagnostics("app.php").await;
    expect!["<empty>"].assert_eq(&render_pull_diagnostics(&resp));

    s.wait_for_index_ready_secs(30).await;
    let resp = s.pull_diagnostics("app.php").await;
    expect!["2:4-2:21 [1] UndefinedClass: Class TrulyMissingClass does not exist"]
        .assert_eq(&render_pull_diagnostics(&resp));
}

/// Rootless (single-file, no workspace) sessions have nothing to scan, so
/// `is_index_ready()` must be true immediately — guards against the gate in
/// `compute_open_file_diagnostics` reintroducing a suppression window here
/// too (there is no `indexReady` event to end it in this mode).
#[tokio::test]
async fn rootless_session_has_no_suppression_window() {
    let mut s = TestServer::new().await;
    let notif = s.open("app.php", "<?php\n\nnonexistent_fn();\n").await;
    expect!["2:0-2:16 [1] UndefinedFunction: Function nonexistent_fn() is not defined"]
        .assert_eq(&render_diagnostics_notification(&notif));
}

/// Same-namespace bare class reference (no `use`) must not emit UndefinedClass.
#[tokio::test]
async fn same_namespace_bare_ref_not_flagged_as_undefined_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/Producer.php"),
        "<?php\nnamespace App;\nclass Producer {\n    public function make(): string { return 'p'; }\n}\n",
    )
    .unwrap();

    // Consumer references Producer in three positions (type hint, new,
    // instanceof) — all bare, no `use` because both live in `namespace App`.
    let consumer_src = "<?php\nnamespace App;\nclass Consumer {\n    public function __construct(private Producer $p) {}\n    public function fresh(): Producer {\n        return new Producer();\n    }\n    public function isProducer(mixed $x): bool {\n        return $x instanceof Producer;\n    }\n}\n";
    std::fs::write(tmp.path().join("src/Consumer.php"), consumer_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Consumer.php", consumer_src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Consumer.php
          <clean>"#]]
    .assert_eq(&out);
}

/// Same-namespace `extends` across files with no `use` must not emit UndefinedClass.
#[tokio::test]
async fn same_namespace_extends_not_flagged_as_undefined_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/Base.php"),
        "<?php\nnamespace App;\nabstract class Base {}\n",
    )
    .unwrap();

    let child_src = "<?php\nnamespace App;\nfinal class Child extends Base {}\n";
    std::fs::write(tmp.path().join("src/Child.php"), child_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Child.php", child_src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Child.php
          <clean>"#]]
    .assert_eq(&out);
}

/// Positive control for the above: a truly-missing same-namespace class must
/// still be flagged. Without this, the no-false-positive tests prove nothing.
#[tokio::test]
async fn undefined_class_in_new() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function _wrap(): void {
    $x = new UnknownClass();
//           ^^^^^^^^^^^^ error: UnknownClass
}
"#,
    )
    .await;
}

#[tokio::test]
async fn undefined_function_detected_in_arrow_function() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
$fn = fn() => nonexistent_function();
//            ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
"#,
        )
        .await;
}

#[tokio::test]
async fn undefined_function_detected_in_closure() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
$fn = function(): void {
    nonexistent_function();
//  ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
};
"#,
        )
        .await;
}

#[tokio::test]
async fn undefined_function_detected_in_static_method() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
class Factory {
    public static function build(): void {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
    }
}
"#,
        )
        .await;
}

#[tokio::test]
async fn undefined_function_detected_in_trait_method() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
trait Auditable {
    public function audit(): void {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
    }
}
"#,
        )
        .await;
}

#[tokio::test]
async fn undefined_function_inside_function() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function wrapper(): void {
    nonexistent_fn();
//  ^^^^^^^^^^^^^^^^ error: nonexistent_fn
}
"#,
    )
    .await;
}

#[tokio::test]
async fn undefined_function_inside_method() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class C {
    public function run(): void {
        nonexistent_fn();
//      ^^^^^^^^^^^^^^^^ error: nonexistent_fn
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn undefined_function_inside_namespaced_method() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
namespace LspTest;
class Broken {
    public function f(): void {
        nonexistent_fn();
//      ^^^^^^^^^^^^^^^^ error: nonexistent_fn
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn undefined_function_top_level() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function _wrap(): void {
    nonexistent_fn();
//  ^^^^^^^^^^^^^^^^ error: nonexistent_fn
}
"#,
    )
    .await;
}

#[tokio::test]
async fn use_imported_interface_in_implements_not_flagged_as_undefined_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Contract")).unwrap();
    std::fs::write(
        tmp.path().join("src/Contract/Runnable.php"),
        "<?php\nnamespace App\\Contract;\ninterface Runnable { public function run(): void; }\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Contract\\Runnable;\nclass Worker implements Runnable { public function run(): void {} }\n";
    std::fs::write(tmp.path().join("src/Service/Worker.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Worker.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Worker.php
          <clean>"#]]
    .assert_eq(&out);
}

// ── UndefinedMethod on trait-aliased methods ─────────────────────────────────

/// Control: a plain (un-aliased) trait method call produces no diagnostics.
#[tokio::test]
async fn trait_method_call_no_false_undefined_method() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
trait BaseInit {
    public function init(int $x): void {}
}
class Query {
    use BaseInit;
    public function __construct() {
        $this->init(1);
    }
}
"#,
    )
    .await;
}

/// Calling a method that does not exist produces UndefinedMethod.
#[tokio::test]
async fn undefined_method_on_known_class_fires() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class Greeter {
    public function hello(): void {}
}
function test(Greeter $g): void {
    $g->doesNotExist();
//  ^^^^^^^^^^^^^^^^^^ error: doesNotExist
}
"#,
    )
    .await;
}

/// Calling a genuinely missing method on a trait-using class still fires UndefinedMethod.
#[tokio::test]
async fn undefined_method_fires_for_class_using_trait() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
trait HasHello {
    public function hello(): void {}
}
class Foo {
    use HasHello;
}
function test(Foo $f): void {
    $f->reallyMissing();
//  ^^^^^^^^^^^^^^^^^^^ error: reallyMissing
}
"#,
    )
    .await;
}

/// `use function` importing a namespaced function (the common Laravel/
/// Symfony helper-file pattern: `function_exists()`-guarded declarations
/// inside a namespace, imported unqualified elsewhere) must not be flagged
/// `UndefinedFunction`. No prior test covered `use function` resolution at all.
#[tokio::test]
async fn use_function_namespaced_import_not_flagged_as_undefined() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
namespace App\Helpers;
if (! function_exists('App\Helpers\greet')) {
    function greet($name) {
        return $name;
    }
}
namespace App;
use function App\Helpers\greet;
function run() {
    return greet("world");
}
"#,
    )
    .await;
}

/// Same as above but across files: the function is declared in one file and
/// imported via `use function` in another, mirroring how Laravel's
/// `autoload.files` helpers (e.g. `enum_value()`) are declared and consumed.
#[tokio::test]
async fn use_function_namespaced_import_not_flagged_as_undefined_cross_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"},"files":["src/Helpers/functions.php"]}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Helpers")).unwrap();
    std::fs::write(
        tmp.path().join("src/Helpers/functions.php"),
        "<?php\nnamespace App\\Helpers;\nif (! function_exists('App\\Helpers\\greet')) {\n    function greet($name) {\n        return $name;\n    }\n}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse function App\\Helpers\\greet;\nfunction run() {\n    return greet(\"world\");\n}\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready_secs(30).await;
    let diag = s.open("src/Service/Handler.php", src).await;
    let empty = vec![];
    let all = diag["params"]["diagnostics"].as_array().unwrap_or(&empty);
    assert!(all.is_empty(), "expected no diagnostics, got: {all:#?}");
}

/// A trait-aliased method call must not be flagged `UndefinedMethod`.
#[tokio::test]
async fn trait_aliased_method_call_no_false_undefined_method() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
trait BaseInit {
    public function __construct(int $x) {}
}
class Query {
    use BaseInit { __construct as __constructBase; }
    public function __construct() {
        $this->__constructBase(1);
    }
}
"#,
    )
    .await;
}

// ── namespace fallback: functions fall back to global, classes never do ─────
//
// PHP resolves an unqualified function/constant call inside a namespace by
// first looking for `CurrentNamespace\name`, then falling back to the global
// namespace if that isn't declared. Unqualified *class* names get no such
// fallback — they always resolve within the current namespace (or via `use`).
// These two behaviors must be asymmetric; testing only one would not catch a
// regression that accidentally applies (or removes) fallback for the other.

/// A global-namespace function called unqualified from inside a different
/// namespace, with no `use function` import, must not be flagged
/// `UndefinedFunction` — PHP falls back to the global function at runtime.
#[tokio::test]
async fn unqualified_call_falls_back_to_global_function() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
namespace {
    function global_only_helper(int $x): int { return $x * 2; }
}

namespace App {
    function run(int $x): int {
        return global_only_helper($x);
    }
}
"#,
    )
    .await;
}

/// Contrast: a global-namespace *class* referenced unqualified from inside a
/// different namespace, with no `use` import, must still be flagged
/// `UndefinedClass` — classes do not fall back to the global namespace.
#[tokio::test]
async fn unqualified_class_does_not_fall_back_to_global() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
namespace {
    class GlobalOnlyThing {}
}

namespace App {
    function make(): void {
        $x = new GlobalOnlyThing();
    //           ^^^^^^^^^^^^^^^ error: GlobalOnlyThing
    }
}
"#,
    )
    .await;
}

/// A leading-backslash reference to a global function from inside a
/// namespace is the explicit, unambiguous form of the fallback above and
/// must not be flagged either.
#[tokio::test]
async fn fully_qualified_global_function_call_from_namespace_not_flagged() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
namespace {
    function global_only_helper(int $x): int { return $x * 2; }
}

namespace App {
    function run(int $x): int {
        return \global_only_helper($x);
    }
}
"#,
    )
    .await;
}

// ── composer.json discovered by walking up from a subdirectory root ────────

/// When the workspace root is a subdirectory (e.g. the editor opened `src/`
/// directly) and `composer.json` actually lives one level up, PSR-4
/// resolution must still find it via `find_composer_root`'s parent-directory
/// walk. No prior test rooted the server below the composer.json itself.
#[tokio::test]
async fn composer_json_found_by_walking_up_from_subdirectory_root() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Entity.php"),
        "<?php\nnamespace App\\Model;\nclass Entity {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity;\nfunction handle(Entity $e): Entity { return $e; }\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    // Root the server at `src/`, one level *below* composer.json.
    let mut s = TestServer::with_root(tmp.path().join("src")).await;
    s.open("Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

// ── vendor package autoloaded via `classmap` only (no PSR-4) ────────────────

/// A vendor package that declares only `classmap` autoload (no `psr-4`) is
/// resolved through `Psr4Map`'s classmap branch — populated from Composer's
/// generated `vendor/composer/autoload_classmap.php`, not from
/// `installed.json` directly. Every other vendor-package test in this suite
/// uses `psr-4`; this is the only coverage of the classmap-only vendor shape
/// at the protocol level.
#[tokio::test]
async fn vendor_classmap_only_package_class_not_flagged_as_undefined() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("vendor/composer")).unwrap();
    std::fs::write(
        tmp.path().join("vendor/composer/installed.json"),
        r#"{"packages":[{"name":"acme/legacy","autoload":{"classmap":["src/"]}}]}"#,
    )
    .unwrap();
    // Mirrors what `composer dump-autoload` actually generates — `resolve()`
    // reads this file for classmap FQCN lookups, not `installed.json`.
    std::fs::write(
        tmp.path().join("vendor/composer/autoload_classmap.php"),
        "<?php\n$vendorDir = dirname(__DIR__);\n$baseDir = dirname($vendorDir);\nreturn array(\n    'Acme\\\\Legacy\\\\Widget' => $vendorDir . '/acme/legacy/src/Widget.php',\n);\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("vendor/acme/legacy/src")).unwrap();
    std::fs::write(
        tmp.path().join("vendor/acme/legacy/src/Widget.php"),
        "<?php\nnamespace Acme\\Legacy;\nclass Widget {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse Acme\\Legacy\\Widget;\nfunction handle(): Widget { return new Widget(); }\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready_secs(30).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

// ── false positives: class refs in positions other than `new`/type-hint ────
//
// Every PSR-4-backed "not flagged" test above exercises `new Foo()` or a
// parameter/return type hint. Class names also appear in `catch`,
// `instanceof`, and `ClassName::` static-call position — if the resolver
// only special-cases the first two, these are silently mis-flagged.

/// A single imported exception class named in a `catch` clause must not be
/// flagged `UndefinedClass`.
#[tokio::test]
async fn catch_clause_with_use_imported_class_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Exception")).unwrap();
    std::fs::write(
        tmp.path().join("src/Exception/NotFoundException.php"),
        "<?php\nnamespace App\\Exception;\nclass NotFoundException extends \\Exception {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Exception\\NotFoundException;\nfunction risky(): void {}\nfunction handle(): void {\n    try {\n        risky();\n    } catch (NotFoundException $e) {\n    }\n}\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

/// A multi-catch (`catch (A | B $e)`) naming two use-imported exception
/// classes must not flag either half of the union.
#[tokio::test]
async fn multi_catch_union_with_use_imported_classes_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Exception")).unwrap();
    std::fs::write(
        tmp.path().join("src/Exception/NotFoundException.php"),
        "<?php\nnamespace App\\Exception;\nclass NotFoundException extends \\Exception {}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/Exception/ForbiddenException.php"),
        "<?php\nnamespace App\\Exception;\nclass ForbiddenException extends \\Exception {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Exception\\NotFoundException;\nuse App\\Exception\\ForbiddenException;\nfunction risky(): void {}\nfunction handle(): void {\n    try {\n        risky();\n    } catch (NotFoundException | ForbiddenException $e) {\n    }\n}\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

/// A class implementing two interfaces, both use-imported via a grouped
/// import, must not flag either — every existing `implements` test names
/// only a single interface.
#[tokio::test]
async fn implements_multiple_use_imported_interfaces_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Contract")).unwrap();
    std::fs::write(
        tmp.path().join("src/Contract/Runnable.php"),
        "<?php\nnamespace App\\Contract;\ninterface Runnable { public function run(): void; }\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/Contract/Loggable.php"),
        "<?php\nnamespace App\\Contract;\ninterface Loggable { public function log(): void; }\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Contract\\{Runnable, Loggable};\nclass Worker implements Runnable, Loggable {\n    public function run(): void {}\n    public function log(): void {}\n}\n";
    std::fs::write(tmp.path().join("src/Service/Worker.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Worker.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Worker.php
          <clean>"#]]
    .assert_eq(&out);
}

/// A cross-file `instanceof` check against a use-imported class must not be
/// flagged. The only existing `instanceof` coverage
/// (`same_namespace_bare_ref_not_flagged_as_undefined_class`) uses a bare
/// same-namespace reference; this covers the `use`-imported, cross-file form.
#[tokio::test]
async fn instanceof_with_use_imported_class_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Entity.php"),
        "<?php\nnamespace App\\Model;\nclass Entity {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity;\nfunction check(mixed $x): bool {\n    return $x instanceof Entity;\n}\n";
    std::fs::write(tmp.path().join("src/Service/Checker.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Checker.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Checker.php
          <clean>"#]]
    .assert_eq(&out);
}

/// A fully-qualified static method call (`\App\Model\Entity::create()`, no
/// `new`) must resolve the class in call position, not just in `new`/type-hint
/// position.
#[tokio::test]
async fn fqn_static_call_not_flagged_as_undefined_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Entity.php"),
        "<?php\nnamespace App\\Model;\nclass Entity {\n    public static function create(): self { return new self(); }\n}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nfunction handle(): void {\n    \\App\\Model\\Entity::create();\n}\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

/// The `class_exists()`-guarded conditional-declaration polyfill pattern
/// (common in Composer packages shimming a class only when missing) must not
/// break resolution for the class's own body or its call sites — the class
/// equivalent of `user_polyfill_does_not_break_builtin_restore_error_handler`.
#[tokio::test]
async fn class_exists_guarded_polyfill_not_flagged() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"//- /src/polyfill.php
<?php
if (!class_exists(JsonExtra::class)) {
    interface JsonExtra {}
}

//- /src/main.php
<?php
function _wrap(JsonExtra $x): void {
}
"#,
    )
    .await;
}

// ── false positives: compound type hints ────────────────────────────────────
//
// Every use-imported type-hint test above names a single class. Union
// (`Foo|Bar`) and intersection (`Foo&Bar`) types name more than one — if the
// resolver only checks the first member of a compound type, the others are
// silently mis-flagged.

/// A union-type parameter naming two use-imported classes must not flag
/// either member.
#[tokio::test]
async fn union_type_param_with_two_use_imported_classes_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Foo.php"),
        "<?php\nnamespace App\\Model;\nclass Foo {}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Bar.php"),
        "<?php\nnamespace App\\Model;\nclass Bar {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Model\\Foo;\nuse App\\Model\\Bar;\nfunction handle(Foo|Bar $x): void {\n}\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

/// An intersection-type parameter (PHP 8.1+) naming two use-imported
/// interfaces must not flag either member.
#[tokio::test]
async fn intersection_type_param_with_two_use_imported_interfaces_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Contract")).unwrap();
    std::fs::write(
        tmp.path().join("src/Contract/Countable2.php"),
        "<?php\nnamespace App\\Contract;\ninterface Countable2 { public function count(): int; }\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/Contract/Nameable.php"),
        "<?php\nnamespace App\\Contract;\ninterface Nameable { public function name(): string; }\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Contract\\Countable2;\nuse App\\Contract\\Nameable;\nfunction handle(Countable2&Nameable $x): void {\n}\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

/// A nullable (`?Foo`) parameter type naming a use-imported class must not be
/// flagged — the `?` prefix must not defeat the type-name resolver.
#[tokio::test]
async fn nullable_type_param_with_use_imported_class_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    std::fs::write(
        tmp.path().join("src/Model/Entity.php"),
        "<?php\nnamespace App\\Model;\nclass Entity {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity;\nfunction handle(?Entity $e): void {\n}\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

// ── false positives: PHP 8.1 first-class callable syntax ───────────────────

/// `strlen(...)` (PHP 8.1 first-class callable syntax) referencing a known
/// built-in function must not be flagged `UndefinedFunction` — a naive
/// resolver keyed on "function name immediately followed by `(`" could treat
/// the `...` placeholder as an unusual argument list and mis-handle the call.
#[tokio::test]
async fn first_class_callable_builtin_not_flagged() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
function _wrap(): void {
    $fn = strlen(...);
}
"#,
    )
    .await;
}

/// Same as above but for a project-defined function, proving the first-class
/// callable syntax doesn't only work for stub/built-in functions.
#[tokio::test]
async fn first_class_callable_user_function_not_flagged() {
    let mut s = TestServer::new().await;
    s.check_no_diagnostics(
        r#"<?php
function greet(string $name): string { return "hi $name"; }
function _wrap(): void {
    $fn = greet(...);
}
"#,
    )
    .await;
}

// ── vendor PSR-4 via Composer v1 (array-form) `installed.json` ─────────────

/// Older Composer lockfiles (and some CI caches) write `installed.json` as a
/// bare array rather than `{"packages": [...]}`. Every other vendor-PSR-4
/// test in this suite uses the v2 object form; this is the only protocol
/// coverage of the legacy array form.
#[tokio::test]
async fn vendor_psr4_via_composer_v1_installed_json_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("vendor/composer")).unwrap();
    std::fs::write(
        tmp.path().join("vendor/composer/installed.json"),
        r#"[{"name":"acme/legacy","autoload":{"psr-4":{"Acme\\Legacy\\":"src/"}}}]"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("vendor/acme/legacy/src")).unwrap();
    std::fs::write(
        tmp.path().join("vendor/acme/legacy/src/Widget.php"),
        "<?php\nnamespace Acme\\Legacy;\nclass Widget {}\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Service")).unwrap();
    let src = "<?php\nnamespace App\\Service;\nuse Acme\\Legacy\\Widget;\nfunction handle(): Widget { return new Widget(); }\n";
    std::fs::write(tmp.path().join("src/Service/Handler.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready_secs(30).await;
    s.open("src/Service/Handler.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Service/Handler.php
          <clean>"#]]
    .assert_eq(&out);
}

// ── false positive: enum implementing a use-imported interface ─────────────

/// An `enum` implementing a use-imported interface must not be flagged.
/// Enums have their own collector/analyzer code path in mir (see the
/// `BackedEnumCaseTypeMismatch` known-gap note in
/// `feature_diagnostics_edge_cases.rs`), so interface resolution for classes
/// isn't proof it also works for enums.
#[tokio::test]
async fn enum_implementing_use_imported_interface_not_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Contract")).unwrap();
    std::fs::write(
        tmp.path().join("src/Contract/HasLabel.php"),
        "<?php\nnamespace App\\Contract;\ninterface HasLabel { public function label(): string; }\n",
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
    let src = "<?php\nnamespace App\\Model;\nuse App\\Contract\\HasLabel;\nenum Status implements HasLabel {\n    case Active;\n    case Inactive;\n    public function label(): string {\n        return match ($this) {\n            self::Active => 'Active',\n            self::Inactive => 'Inactive',\n        };\n    }\n}\n";
    std::fs::write(tmp.path().join("src/Model/Status.php"), src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.open("src/Model/Status.php", src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Model/Status.php
          <clean>"#]]
    .assert_eq(&out);
}
