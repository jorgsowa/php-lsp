//! Rename coverage: prepareRename bounds + actual rename across files.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn prepare_rename_on_identifier() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_prepare_rename(
            r#"<?php
function gre$0et(): void {}
"#,
        )
        .await;
    expect!["1:9-1:14"].assert_eq(&out);
}

/// With the cursor on the `$` sigil the range still covers just the name.
#[tokio::test]
async fn prepare_rename_variable_cursor_on_sigil() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_prepare_rename(
            r#"<?php
$0$foo = 1;
"#,
        )
        .await;
    expect!["1:1-1:4"].assert_eq(&out);
}

/// `prepareRename` on a PHP keyword must return null so the editor greys out
/// the rename action rather than presenting an empty rename dialog.
#[tokio::test]
async fn prepare_rename_on_keyword_returns_nothing() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_prepare_rename(
            r#"<?php
func$0tion greet(): void {}
"#,
        )
        .await;
    expect!["<not renameable>"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_rename_on_builtin_function_returns_nothing() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_prepare_rename(
            r#"<?php
$n = str$0len('abc');
"#,
        )
        .await;
    expect!["<not renameable>"].assert_eq(&out);
}

#[tokio::test]
async fn rename_builtin_function_in_namespace_makes_no_edits() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_rename(
            r#"//- /a.php
<?php
namespace App;
$n = str$0len('abc');

//- /b.php
<?php
$m = strlen('xyz');
"#,
            "mylen",
        )
        .await;
    expect!["<no `changes` map in null>"].assert_eq(&out);
}

/// `parent`, `self`, and `static` are PHP class-reference keywords and must
/// not be renameable even though they look like identifiers.
#[tokio::test]
async fn prepare_rename_on_parent_self_static_returns_nothing() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    for (src, label) in &[
        (
            "<?php\nclass Child extends Base {\n    public function f(): void { par$0ent::f(); }\n}",
            "parent",
        ),
        (
            "<?php\nclass Foo {\n    public static function make(): static { return se$0lf::create(); }\n}",
            "self",
        ),
        (
            "<?php\nclass Foo {\n    public static function create(): sta$0tic { return new static(); }\n}",
            "static",
        ),
    ] {
        let out = s.check_prepare_rename(src).await;
        assert_eq!(
            out, "<not renameable>",
            "prepare_rename should block {label}"
        );
    }
}

/// PHP allows almost every keyword as a method name (`public function
/// match(): void {}` is valid PHP). Both the declaration and a `->` call
/// site must remain renameable — only the bare keyword-as-keyword case
/// (covered by `prepare_rename_on_keyword_returns_nothing`) should be
/// blocked.
#[tokio::test]
async fn prepare_rename_on_keyword_named_method_is_allowed() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);

    let decl_out = s
        .check_prepare_rename(
            r#"<?php
class Handler {
    public function mat$0ch(): void {}
}
"#,
        )
        .await;
    assert_ne!(
        decl_out, "<not renameable>",
        "a method declaration named `match` must be renameable"
    );

    let call_out = s
        .check_prepare_rename(
            r#"<?php
class Handler {
    public function match(): void {}
}
function use_it(Handler $h): void {
    $h->mat$0ch();
}
"#,
        )
        .await;
    assert_ne!(
        call_out, "<not renameable>",
        "a `->match()` call site must be renameable"
    );

    let static_out = s
        .check_prepare_rename(
            r#"<?php
class Handler {
    public static function match(): void {}
}
function use_it(): void {
    Handler::mat$0ch();
}
"#,
        )
        .await;
    assert_ne!(
        static_out, "<not renameable>",
        "a `Handler::match()` static call site must be renameable"
    );

    // Trailing-arrow wrap style — `->` ends the receiver's line, so the
    // method name's own line carries only leading whitespace before it.
    let wrapped_out = s
        .check_prepare_rename(
            r#"<?php
class Handler {
    public function match(): void {}
}
function use_it(Handler $h): void {
    $h->
        mat$0ch();
}
"#,
        )
        .await;
    assert_ne!(
        wrapped_out, "<not renameable>",
        "a `->match()` call site wrapped across lines must be renameable"
    );
}

/// `prepareRename` on a variable should return the range covering the
/// variable name (without `$`) so editors highlight the right text.
#[tokio::test]
async fn prepare_rename_on_variable() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_prepare_rename(
            r#"<?php
function f(): void {
    $cou$0nt = 0;
}
"#,
        )
        .await;
    expect!["2:5-2:10"].assert_eq(&out);
}

#[tokio::test]
async fn rename_on_nonexistent_symbol_does_not_error() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.open("rn.php", "<?php\n// nothing to rename\n").await;
    let resp = s.rename("rn.php", 1, 5, "NewName").await;
    let snap = canonicalize_workspace_edit(&resp["result"], &s.uri(""));
    expect![[r#""#]].assert_eq(&snap);
}

// --- psr4-mini fixture: cross-file rename + PSR4-aware file rename ---

/// Set up psr4-mini with all three files open in the document store.
/// Both the in-file rename and willRenameFiles handlers require open documents.
async fn psr4_bring_up() -> TestServer {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;
    let (user, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.open("src/Model/User.php", &user).await;
    let (reg, _, _) = server.locate("src/Service/Registry.php", "<?php", 0);
    server.open("src/Service/Registry.php", &reg).await;
    let (greet, _, _) = server.locate("src/Service/Greeter.php", "<?php", 0);
    server.open("src/Service/Greeter.php", &greet).await;
    server
}

/// Renaming `class User` to `Account` must rewrite every `use App\Model\User`
/// import in dependent files. Snapshot-pinned so byte-offset regressions are
/// caught immediately.
#[tokio::test]
async fn rename_class_edits_all_dependents() {
    let mut server = psr4_bring_up().await;
    let (_, line, ch) = server.locate("src/Model/User.php", "class User", 0);

    let resp = server
        .rename("src/Model/User.php", line, ch + 6, "Account")
        .await;

    assert!(resp["error"].is_null(), "rename error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Model/User.php
        4:6-4:10 → "Account"

        // src/Service/Greeter.php
        4:14-4:18 → "Account"
        8:26-8:30 → "Account"

        // src/Service/Registry.php
        4:14-4:18 → "Account"
        11:29-11:33 → "Account""#]]
    .assert_eq(&snap);
}

/// Moving `src/Model/User.php` to `src/Entity/User.php` changes the FQN from
/// `App\Model\User` to `App\Entity\User`; every `use App\Model\User` must be
/// rewritten.
#[tokio::test]
async fn will_rename_file_rewrites_use_imports_in_dependents() {
    let mut server = psr4_bring_up().await;
    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Entity/User.php");

    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;

    assert!(resp["error"].is_null(), "willRenameFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Service/Greeter.php
        4:4-4:18 → "App\\Entity\\User"

        // src/Service/Registry.php
        4:4-4:18 → "App\\Entity\\User""#]]
    .assert_eq(&snap);
}

/// Renaming a file to the same PSR4-derived FQN must be a no-op.
#[tokio::test]
async fn will_rename_file_same_psr4_fqn_produces_no_edits() {
    let mut server = psr4_bring_up().await;
    let old_uri = server.uri("src/Model/User.php");
    let new_uri = old_uri.clone();

    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;
    assert!(resp["error"].is_null(), "willRenameFiles error: {resp:?}");
    let snap = canonicalize_workspace_edit(&resp["result"], &server.uri(""));
    expect!["<no `changes` map in null>"].assert_eq(&snap);
}

/// Deleting the file that defines `App\Model\User` must strip the `use` line
/// from every dependent.
#[tokio::test]
async fn will_delete_file_strips_use_imports_from_dependents() {
    let mut server = psr4_bring_up().await;
    let uri = server.uri("src/Model/User.php");

    let resp = server.will_delete_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willDeleteFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Service/Greeter.php
        4:0-5:0 → ""

        // src/Service/Registry.php
        4:0-5:0 → """#]]
    .assert_eq(&snap);
}

/// **LIMITATION**: Callable/closure parameter types are not fully supported.
/// Type hints like `callable`, `Closure`, etc. don't resolve to actual type definitions.
#[tokio::test]
async fn rename_limitation_callable_types() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_rename(
            r#"<?php
function process(callable $callback$0): void {
    $callback();
}
"#,
            "$handler",
        )
        .await;
    // Rename the parameter itself works
    expect![[r#"
        // main.php
        1:26-1:35 → "$handler"
        2:4-2:13 → "$handler""#]]
    .assert_eq(&out);
}

/// Superglobals ($_GET, $_POST, etc.) are part of the PHP runtime; renaming
/// them breaks code, so `prepare_rename` returns null to disable the action.
#[tokio::test]
async fn rename_superglobal_is_blocked_by_prepare_rename() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    for superglobal in &[
        "$_GET",
        "$_POST",
        "$_REQUEST",
        "$_FILES",
        "$_COOKIE",
        "$_SESSION",
        "$_SERVER",
        "$_ENV",
        "$GLOBALS",
    ] {
        let src = format!("<?php\necho {superglobal}$0['key'];\n");
        let out = s.check_prepare_rename(&src).await;
        assert_eq!(
            out, "<not renameable>",
            "prepare_rename should block {superglobal}"
        );
    }
}

/// `$this` is PHP's object-context pseudo-variable and cannot be renamed.
#[tokio::test]
async fn rename_this_is_blocked_by_prepare_rename() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_prepare_rename(
            r#"<?php
class Foo {
    public function bar(): void { $th$0is->baz(); }
}
"#,
        )
        .await;
    expect!["<not renameable>"].assert_eq(&out);
}

// ── variable rename: scope boundaries ────────────────────────────────────────

/// Edge case: UTF-16 multibyte character handling in FQN.
/// PHP supports Unicode identifiers. This test verifies that rename correctly
/// calculates character positions when the FQN contains multibyte characters.
/// Critical: must use UTF-16 code unit offsets, not byte offsets.
/// Bug scenario: If using raw byte length instead of UTF-16 length, the end
/// position would be wrong and the rename would truncate or extend incorrectly.
#[tokio::test]
async fn rename_multibyte_unicode_in_use_statement() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    // FQN "App\Été\OldName": contains Unicode characters É and é
    // Each is 1 UTF-16 code unit but 2 bytes in UTF-8
    let out = s
        .check_rename(
            r#"<?php
use App\Été\OldName;
$obj = new OldName$0();
"#,
            "NewName",
        )
        .await;
    // Should correctly rename only the class name part (OldName → NewName)
    // The use statement line should show the replacement position accounting for UTF-16
    expect![[r#"
        // main.php
        1:12-1:19 → "NewName"
        2:11-2:18 → "NewName""#]]
    .assert_eq(&out);
}

/// Renaming a promoted constructor parameter from its declaration site should
/// update the declaration and all property access sites.
#[tokio::test]
async fn rename_promoted_property_from_declaration_site() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_rename(
            r#"<?php
class Point {
    public function __construct(
        public readonly float $la$0t,
        public readonly float $lng,
    ) {}
    public function label(): string { return (string) $this->lat; }
}
"#,
            "latitude",
        )
        .await;
    expect![[r#"
        // main.php
        3:31-3:34 → "latitude"
        6:61-6:64 → "latitude""#]]
    .assert_eq(&out);
}

/// Renaming a property from a trait declaration site should work identically
/// to renaming from a class declaration site.
#[tokio::test]
async fn rename_property_from_trait_declaration_site() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_rename(
            r#"<?php
trait HasTimestamps {
    public ?\DateTimeImmutable $create$0dAt;
}
class Post {
    use HasTimestamps;
    public function touch(): void { $this->createdAt = new \DateTimeImmutable(); }
}
"#,
            "createdAtUtc",
        )
        .await;
    expect![[r#"
        // main.php
        2:32-2:41 → "createdAtUtc"
        6:43-6:52 → "createdAtUtc""#]]
    .assert_eq(&out);
}

// ── clone-with (PHP 8.5) ────────────────────────────────────────────────────

/// PHP magic constants (__CLASS__, __FILE__, etc.) are compiler-generated and
/// cannot be renamed. `prepareRename` must return nothing for all of them.
#[tokio::test]
async fn prepare_rename_on_magic_constants_returns_nothing() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    for (src, label) in &[
        (
            "<?php\nclass Foo { public function bar(): void { echo __CL$0ASS__; } }",
            "__CLASS__",
        ),
        ("<?php\necho __FI$0LE__;", "__FILE__"),
        ("<?php\necho __DI$0R__;", "__DIR__"),
        (
            "<?php\nfunction f() { return __FUNC$0TION__; }",
            "__FUNCTION__",
        ),
        (
            "<?php\nclass Foo { public function bar() { return __METH$0OD__; } }",
            "__METHOD__",
        ),
        (
            "<?php\nnamespace App; echo __NAMES$0PACE__;",
            "__NAMESPACE__",
        ),
        (
            "<?php\ntrait T { public function f() { echo __TR$0AIT__; } }",
            "__TRAIT__",
        ),
        ("<?php\necho __LI$0NE__;", "__LINE__"),
    ] {
        let out = s.check_prepare_rename(src).await;
        assert_eq!(
            out, "<not renameable>",
            "prepare_rename should block {label}"
        );
    }
}

/// PHP soft reserved words (type keywords) cannot be used as class or function
/// names in PHP 7+. `prepareRename` must block them so a misclick on a type
/// hint does not launch a rename that would corrupt the file.
#[tokio::test]
async fn prepare_rename_on_type_keywords_returns_nothing() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    for (src, label) in &[
        ("<?php\nfunction f(i$0nt $x): void {}", "int"),
        ("<?php\nfunction f(flo$0at $x): void {}", "float"),
        ("<?php\nfunction f(bo$0ol $x): void {}", "bool"),
        ("<?php\nfunction f(str$0ing $x): void {}", "string"),
        ("<?php\nfunction f(): vo$0id {}", "void"),
        (
            "<?php\nfunction f(): nev$0er { throw new \\Exception(); }",
            "never",
        ),
        ("<?php\nfunction f(): mix$0ed {}", "mixed"),
        ("<?php\nfunction f(): obj$0ect {}", "object"),
        ("<?php\nfunction f(iterab$0le $x): void {}", "iterable"),
    ] {
        let out = s.check_prepare_rename(src).await;
        assert_eq!(
            out, "<not renameable>",
            "prepare_rename should block type keyword {label}"
        );
    }
}

/// A typed, defaulted parameter keeps its type and default.
#[tokio::test]
async fn rename_typed_param_edits_only_the_name() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_rename(
            "<?php\nfunction foo(?int $ol$0d = null, string $oldest = '$old') { return $old; }\n",
            "new",
        )
        .await;
    expect![[r#"
        // main.php
        1:18-1:22 → "$new"
        1:65-1:69 → "$new""#]]
    .assert_eq(&out);
}

/// Calls after an emoji are renamed at their UTF-16 columns, not skipped.
#[tokio::test]
async fn rename_function_called_on_emoji_line() {
    let mut s = TestServer::new().await;
    s.open(
        "main.php",
        "<?php\nfunction bump(): void {}\necho '😀'; bump();\n",
    )
    .await;
    let resp = s.rename("main.php", 1, 10, "make").await;
    expect![[r#"
        // main.php
        1:9-1:13 → "make"
        2:11-2:15 → "make""#]]
    .assert_eq(&canonicalize_workspace_edit(&resp["result"], &s.uri("")));
}
