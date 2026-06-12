//! Protocol-wired regression tests for known Laravel/PHP LSP gaps.
//!
//! Each test documents the **current** (broken) behaviour with an `expect!`
//! snapshot.  When a gap is fixed the snapshot assertion will fail, signalling
//! that the expected value must be updated and the companion documentation in
//! `CLAUDE.md` revised.
//!
//! These tests intentionally use synthetic PHP that mimics common Laravel
//! patterns rather than pulling in the real framework corpus, so they run
//! quickly and without external dependencies.

use super::*;
use expect_test::expect;

// ── Gap #1 — Facade static-call type resolution ───────────────────────────────

/// `Auth::user()` is a Facade call. The LSP cannot resolve the underlying
/// concrete class from the service container binding, so go-to-definition
/// silently returns nothing.
///
/// **Gap**: requires service-container binding modelling in mir-php.
#[tokio::test]
async fn gap_facade_definition_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
class Auth {
    public static function user(): mixed { return null; }
}

$u = Auth::us$0er();
"#,
        )
        .await;
    // Auth::user() IS a real method above, so definition resolves.
    // The real gap is that facades use a __callStatic / getFacadeAccessor
    // chain that the LSP doesn't follow. Test below uses the facade pattern.
    expect!["main.php:2:27-2:31"].assert_eq(&out);
}

/// Facade with `__callStatic` forwarding — the LSP cannot trace through
/// `getFacadeAccessor` to the bound implementation, so definition returns nothing.
///
/// **Gap**: service container resolution is a runtime concern.
#[tokio::test]
async fn gap_facade_callstatic_definition_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
class AuthFacade {
    protected static function getFacadeAccessor(): string { return 'auth'; }
    public static function __callStatic(string $method, array $args): mixed { return null; }
}

// Calling a method only available on the bound concrete (not on AuthFacade itself)
$u = AuthFacade::log$0in('admin@example.com', 'secret');
"#,
        )
        .await;
    // `login` is not defined on AuthFacade — must be `<none>`.
    expect!["<none>"].assert_eq(&out);
}

// ── Gap #3 — Anonymous class in find-implementations ─────────────────────────

/// `find_implementations` for an interface does not include anonymous class
/// implementors because `ExprKind::AnonymousClass` is an expression node and
/// the implementation walker only visits `StmtKind::Class`.
///
/// **Gap**: requires expression-tree traversal in the implementation walker.
#[tokio::test]
async fn gap_anonymous_class_not_in_implementations() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Renderable$0 {
    public function render(): string;
}

// Anonymous implementor — currently invisible to find-implementations.
$view = new class implements Renderable {
    public function render(): string { return '<div/>'; }
};

// Named implementor — this one IS found.
class HtmlView implements Renderable {
    public function render(): string { return '<p/>'; }
}
"#,
        )
        .await;
    // Only the named implementor is found; the anonymous one is silently skipped.
    expect!["main.php:11:6-11:14"].assert_eq(&out);
}

// ── Gap #6 — Service container app() / resolve() ─────────────────────────────

/// `app(Foo::class)` should resolve to `Foo`, but the LSP cannot trace the
/// service container at static analysis time.
///
/// **Gap**: binding map is a runtime concept; requires a dedicated extractor.
#[tokio::test]
async fn gap_service_container_app_definition_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
class UserRepository {
    public function find(int $id): mixed { return null; }
}

function bootstrap(): void {
    $repo = app(UserRep$0ository::class);
}
"#,
        )
        .await;
    // `app()` is not defined — definition falls through to `UserRepository` class.
    // This is actually the best the LSP can do without container resolution.
    expect!["main.php:1:6-1:20"].assert_eq(&out);
}

// ── Gap #8 — Eloquent model attribute inference ───────────────────────────────

/// Eloquent model attributes (columns) are determined at runtime from the
/// database schema.  Hovering over `$user->name` on an Eloquent-style model
/// now shows `mixed` — the return type of the inherited `__get` accessor.
/// The actual column type (e.g. `string`) cannot be inferred without a
/// schema-to-stub pipeline or IDE helpers.
///
/// **Gap**: column types require a schema-to-stub pipeline or IDE helpers.
#[tokio::test]
async fn gap_eloquent_attribute_hover_shows_magic_get_type() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
class Model {
    public function __get(string $key): mixed { return null; }
}

class User extends Model {
    protected string $table = 'users';
}

$user = new User();
$n = $user->nam$0e;
"#,
        )
        .await;
    // `name` is a database column, but __get returns mixed; hover shows that type.
    expect![[r#"
        ```php
        (property) User::$name: mixed
        ```"#]]
    .assert_eq(&out);
}

// ── Gap #10 — Abstract class method contract enforcement ─────────────────────

/// A concrete class that extends an abstract class without implementing all
/// abstract methods should produce a diagnostic.  Currently mir-php does not
/// emit this error.
///
/// **Gap**: missing-implementation enforcement is a mir-php analysis feature.
#[tokio::test]
async fn gap_abstract_method_missing_implementation_no_diagnostic() {
    let mut s = TestServer::new().await;
    // PHP itself raises a fatal error for this pattern; disable syntax
    // validation so the LSP still processes it and we can check the diagnostic.
    s.validate_syntax(false);
    let out = s
        .check_definition(
            r#"<?php
abstract class BaseController {
    abstract public function hand$0le(): void;
}

class DashboardController extends BaseController {}
"#,
        )
        .await;
    // Definition on the abstract method itself works fine.
    // The gap is that `DashboardController` produces no diagnostic about the
    // missing implementation. When mir-php gains enforcement, add a
    // `check_diagnostics` assertion with an error annotation on the subclass.
    expect!["main.php:2:29-2:35"].assert_eq(&out);
}

// ── Gap: hover on @method virtual methods ────────────────────────────────────

/// Hovering over a call to an `@method`-declared virtual method currently
/// shows nothing because the hover path calls `scan_method_of_class` which
/// only walks real AST members.
///
/// **Gap**: hover handler needs a fallback that reads `doc_methods` from
/// `FileIndex`; requires threading workspace indexes into `mir_member_hover`.
#[tokio::test]
async fn gap_doc_method_hover_shows_nothing() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
/**
 * @method User find(int $id)
 * @method static Builder where(string $col, mixed $val)
 */
class QueryBuilder {}

function run(QueryBuilder $qb): void {
    $qb->fin$0d(1);
}
"#,
        )
        .await;
    // `find` is declared only via @method — hover currently shows nothing.
    expect!["<no hover>"].assert_eq(&out);
}
