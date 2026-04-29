//! Diagnostic coverage matrix using the caret annotation DSL.
//! Each test names the expectation inline with `// ^^^ severity: message`.

mod common;

use common::TestServer;

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

/// Regression for issue #170: mir-analyzer must detect errors inside
/// namespaced class method bodies, not just top-level / non-namespaced code.
#[tokio::test]
async fn issue_170_errors_inside_namespaced_method_detected() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
namespace LspTest;

class Broken
{
    public int $count = 0;

    public function bump(): int
    {
        $this->count++;
        return $this->count;
    }

    public function obviouslyBroken(): int
    {
        nonexistent_function();
//      ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
        $x = new UnknownClass();
//               ^^^^^^^^^^^^ error: UnknownClass
        return 0;
    }
}
"#,
    )
    .await;
}

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
async fn clean_file_has_no_diagnostics() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function f(string $x): string { return $x; }
f('ok');
"#,
    )
    .await;
}

#[tokio::test]
async fn diagnostics_clear_after_fix() {
    let mut s = TestServer::new().await;
    let notif = s.open("fix.php", "<?php\nundefined_fn();\n").await;
    assert!(
        !notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty()
    );
    let after = s.change("fix.php", 2, "<?php\n").await;
    assert!(
        after["params"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn parse_error_emits_diagnostic() {
    let mut s = TestServer::new().await;
    let notif = s.open("bad.php", "<?php\nfunction f( {\n").await;
    assert!(
        !notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected parse diagnostic for malformed PHP"
    );
}

#[tokio::test]
async fn multiple_diagnostics_same_file() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function _wrap(): void {
    one_undefined();
//  ^^^^^^^^^^^^^^^ error: one_undefined
    two_undefined();
//  ^^^^^^^^^^^^^^^ error: two_undefined
}
"#,
    )
    .await;
}

#[tokio::test]
async fn pull_diagnostics_returns_report() {
    let mut server = TestServer::new().await;
    server.open("pull_diag.php", "<?php\n$x = 1;\n").await;

    let resp = server.pull_diagnostics("pull_diag.php").await;

    assert!(
        resp["error"].is_null(),
        "textDocument/diagnostic error: {:?}",
        resp
    );
    let result = &resp["result"];
    assert!(!result.is_null(), "expected non-null diagnostic report");
    // First pull on a freshly-opened file must be a full report, not unchanged.
    assert_eq!(
        result["kind"].as_str(),
        Some("full"),
        "first pull must return kind='full', got: {:?}",
        result["kind"]
    );
    // Clean file has no diagnostics.
    let items = result["items"]
        .as_array()
        .expect("'items' array in full report");
    assert!(
        items.is_empty(),
        "clean file should have zero diagnostics, got: {items:?}"
    );
}

#[tokio::test]
async fn workspace_diagnostic_returns_report() {
    let mut server = TestServer::new().await;
    server.open("ws_diag.php", "<?php\n$x = 1;\n").await;

    let resp = server.workspace_diagnostic().await;

    assert!(
        resp["error"].is_null(),
        "workspace/diagnostic error: {:?}",
        resp
    );
    let result = &resp["result"];
    let items = result["items"]
        .as_array()
        .expect("expected 'items' array in workspace diagnostic report");
    assert_eq!(
        items.len(),
        1,
        "expected exactly one item for the one opened file, got: {items:?}"
    );
}

#[tokio::test]
async fn requests_on_parse_error_file_do_not_error() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("broken.php", "<?php\nfunction f( $x { // missing ): body\n")
        .await;

    let diags = notif["params"]["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !diags.is_empty(),
        "expected parse diagnostics for broken source"
    );

    let resp = server.hover("broken.php", 1, 10).await;
    assert!(resp["error"].is_null(), "hover errored: {resp:?}");

    let resp = server.document_symbols("broken.php").await;
    assert!(resp["error"].is_null(), "documentSymbol errored: {resp:?}");

    let resp = server.folding_range("broken.php").await;
    assert!(resp["error"].is_null(), "foldingRange errored: {resp:?}");
}

#[tokio::test]
async fn diagnostics_published_on_did_change_for_undefined_function() {
    let mut server = TestServer::new().await;
    server.open("change_test.php", "<?php\n").await;

    let notif = server
        .change("change_test.php", 2, "<?php\nnonexistent_function();\n")
        .await;
    let has = notif["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["code"].as_str() == Some("UndefinedFunction"));
    assert!(has, "expected UndefinedFunction after didChange: {notif:?}");
}

/// Regression for issue #177 — deprecated-call warnings must appear on did_open,
/// not only after the first did_change.
#[tokio::test]
async fn did_open_reports_deprecated_call_warning() {
    let mut server = TestServer::new().await;
    let notif = server
        .open(
            "deprecated_test.php",
            "<?php\n/** @deprecated Use newFunc() instead */\nfunction oldFunc(): void {}\n\noldFunc();\n",
        )
        .await;
    let diags = notif["params"]["diagnostics"].as_array().unwrap();
    let hit = diags.iter().find(|d| {
        d["code"].as_str() == Some("DeprecatedCall")
            && d["message"]
                .as_str()
                .map(|m| m.contains("oldFunc"))
                .unwrap_or(false)
    });
    assert!(
        hit.is_some(),
        "expected DeprecatedCall diagnostic for oldFunc on did_open, got: {diags:?}"
    );
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

#[ignore = "mir-analyzer gap: trait method bodies are not analyzed"]
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
async fn undefined_function_detected_in_closure() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
$fn = function() {
    nonexistent_function();
//  ^^^^^^^^^^^^^^^^^^^^^^ error: nonexistent_function
};
"#,
        )
        .await;
}

#[tokio::test]
async fn argument_count_too_few_detected() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function needs_two(string $a, string $b): void {}
function wrap(): void {
    needs_two('x');
//  ^^^^^^^^^^^^^^ error: needs_two
}
"#,
        )
        .await;
}

#[tokio::test]
async fn argument_type_mismatch_detected() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function takes_string(string $s): void {}
function wrap(): void {
    takes_string(42);
//               ^^ error: takes_string
}
"#,
        )
        .await;
}

/// PSR-4-resolvable classes must not produce UndefinedClass diagnostics even
/// when the background workspace scan has not yet reached the dependency file.
/// The fix (PSR-4 lazy-loading inside `get_semantic_issues_salsa`) reads the
/// dependency from disk before running semantic analysis, making the result
/// deterministic regardless of scan timing.
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
    let notif = s.open("src/Service/Handler.php", handler_src).await;

    let diags = notif["params"]["diagnostics"]
        .as_array()
        .unwrap_or(&vec![])
        .clone();
    assert!(
        diags.is_empty(),
        "expected zero diagnostics for clean PSR-4-resolvable file, got: {diags:?}"
    );
}

#[ignore = "mir-analyzer gap: too-many-arguments not detected"]
#[tokio::test]
async fn argument_count_too_many_detected() {
    let mut server = TestServer::new().await;
    server
        .check_diagnostics(
            r#"<?php
function takes_one(string $s): void {}
function wrap(): void {
    takes_one('a', 'b', 'c');
//  ^^^^^^^^^^^^^^^^^^^^^^^^^ error: takes_one
}
"#,
        )
        .await;
}

/// Regression: `new ShortName()` where `use A\B\ShortName;` must not emit
/// UndefinedClass when the class is on disk (PSR-4 lazy-loading path).
/// Distinct from `psr4_imported_class_not_flagged_before_workspace_scan` which
/// only tested parameter type hints — this exercises the `new` expression path.
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
    let notif = s.open("src/Service/Handler.php", src).await;

    let diags = notif["params"]["diagnostics"]
        .as_array()
        .unwrap_or(&vec![])
        .clone();
    assert!(
        diags.is_empty(),
        "new Entity() must not emit UndefinedClass when class is PSR-4-resolvable; got: {diags:?}"
    );
}

/// Regression: `use A\B\C as Alias; new Alias()` must not emit UndefinedClass.
/// The explicit `as` form writes a different key into `file_imports` than the
/// implicit short-name form, and is the primary path that was broken before
/// mir 0.14.0 populated `Codebase.file_imports` from `StubSlice.imports`.
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
    let notif = s.open("src/Service/Handler.php", src).await;

    let diags = notif["params"]["diagnostics"]
        .as_array()
        .unwrap_or(&vec![])
        .clone();
    let undef: Vec<_> = diags
        .iter()
        .filter(|d| d["code"].as_str() == Some("UndefinedClass"))
        .collect();
    assert!(
        undef.is_empty(),
        "new EntityAlias() must not emit UndefinedClass with explicit `as` alias; got: {undef:?}"
    );
}

/// Sanity baseline: fully-qualified `new \App\Model\Entity()` (no `use` statement)
/// must not emit UndefinedClass when the class is PSR-4-resolvable.
/// Tracked as a known gap: PSR-4 lazy-loading only inspects `use` statements,
/// so FQN `new` expressions that bypass the import list are not resolved.
#[ignore = "mir-analyzer gap: PSR-4 lazy-loading does not cover FQN new expressions"]
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
    let notif = s.open("src/Service/Handler.php", src).await;

    let diags = notif["params"]["diagnostics"]
        .as_array()
        .unwrap_or(&vec![])
        .clone();
    assert!(
        diags.is_empty(),
        "new \\App\\Model\\Entity() (FQN) must not emit UndefinedClass; got: {diags:?}"
    );
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

// ── named argument diagnostics ────────────────────────────────────────────────

#[tokio::test]
async fn duplicate_named_arg_in_function_call() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function foo(int $a, int $b): void {}
foo(a: 1, b: 2, a: 3);
//              ^^^^ error: foo() has no parameter named $a
"#,
    )
    .await;
}

#[tokio::test]
async fn duplicate_named_arg_in_method_call() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class C {
    public function run(int $x, int $y): void {}
}
(new C())->run(x: 1, y: 2, x: 99);
//                         ^^^^^ error: run() has no parameter named $x
"#,
    )
    .await;
}

#[tokio::test]
async fn duplicate_named_arg_in_constructor() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
class Point {
    public function __construct(public int $x, public int $y) {}
}
new Point(x: 0, y: 1, x: 2);
//                    ^^^^ error: Point::__construct() has no parameter named $x
"#,
    )
    .await;
}

#[tokio::test]
async fn positional_after_named_arg() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function bar(int $a, int $b): void {}
bar(a: 1, 2);
//        ^ error: cannot use positional argument after named argument
//        ^ error: bar() has no parameter named $#2
"#,
    )
    .await;
}

#[tokio::test]
async fn valid_named_args_produce_no_diagnostic() {
    let mut s = TestServer::new().await;
    s.check_diagnostics(
        r#"<?php
function greet(string $name, int $times): void {}
greet(name: 'Alice', times: 3);
"#,
    )
    .await;
}
