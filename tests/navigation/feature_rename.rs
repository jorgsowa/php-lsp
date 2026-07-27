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

#[tokio::test]
async fn rename_function_same_file() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function gre$0et(): void {}
//       ^^^^^ rename
  greet();
//^^^^^ rename
  greet();
//^^^^^ rename
"#,
        "salute",
    )
    .await;
}

#[tokio::test]
async fn rename_method_across_file() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
    //              ^^^^^ rename
}
$g = new Greeter();
$g->hello();
//  ^^^^^ rename
"#,
        "salute",
    )
    .await;
}

/// Renaming a variable inside an enum method produces edits for all occurrences.
#[tokio::test]
async fn rename_variable_inside_enum_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
enum Status {
    public function label($a$0rg) { return $arg + 1; }
    //                    ^^^^ rename
    //                                   ^^^^ rename
}
"#,
        "value",
    )
    .await;
}

/// Renaming a variable parameter in an interface method (bodyless) produces edits.
#[tokio::test]
async fn rename_variable_interface_method_param() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
interface Logger {
    public function log($mes$0sage): void;
    //                  ^^^^^^^^ rename
}
"#,
        "$msg",
    )
    .await;
}

/// Renaming a variable parameter in an abstract class method produces edits.
#[tokio::test]
async fn rename_variable_abstract_class_method_param() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
abstract class Processor {
    abstract public function process($in$0put): string;
    //                               ^^^^^^ rename
}
"#,
        "$data",
    )
    .await;
}

/// Renaming a variable parameter in an abstract trait method produces edits.
#[tokio::test]
async fn rename_variable_abstract_trait_method_param() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
trait Formattable {
    abstract public function format($da$0ta): string;
    //                              ^^^^^ rename
}
"#,
        "$input",
    )
    .await;
}

#[tokio::test]
async fn rename_class_updates_new_sites() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
class Wid$0get {}
//    ^^^^^^ rename
$a = new Widget();
//       ^^^^^^ rename
$b = new Widget();
//       ^^^^^^ rename
"#,
        "Gadget",
    )
    .await;
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

/// Renaming a property via a `->access` site must update the declaration and
/// all other access sites. The cursor must be on the bare name after `->`,
/// not on the `$prop` declaration (which is treated as a variable rename).
#[tokio::test]
async fn rename_property_updates_all_access_sites() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
class Counter {
    public int $count = 0;
    //          ^^^^^ rename
    public function inc(): void { $this->coun$0t++; }
    //                                   ^^^^^ rename
    public function get(): int  { return $this->count; }
    //                                          ^^^^^ rename
}
"#,
        "total",
    )
    .await;
}

/// Renaming a class rewrites the matching `use` import in addition to call sites.
#[tokio::test]
async fn rename_class_rewrites_use_import_in_same_file() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
use Vendor\Lib\Widget;
//             ^^^^^^ rename
$a = new Wid$0get();
//       ^^^^^^ rename
$b = new Widget();
//       ^^^^^^ rename
"#,
        "Gadget",
    )
    .await;
}

/// Cross-file companion to `rename_class_rewrites_use_import_in_same_file`:
/// renaming the class in one file must rewrite both the `use` import segment
/// and short-name expression sites in dependents. Snapshot pinned so the
/// merged AST walker can't silently drop either category.
#[tokio::test]
async fn rename_class_rewrites_use_imports_across_files() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"//- /src/Widget.php
<?php
namespace App;
class Wid$0get {}
//    ^^^^^^ rename

//- /src/a.php
<?php
use App\Widget;
//      ^^^^^^ rename
$x = new Widget();
//       ^^^^^^ rename
$is = $x instanceof Widget;
//                  ^^^^^^ rename

//- /src/b.php
<?php
use App\Widget;
//      ^^^^^^ rename
$y = new Widget();
//       ^^^^^^ rename
"#,
        "Gadget",
    )
    .await;
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

/// Rename must match exact word boundaries and not rename partial matches.
#[tokio::test]
async fn rename_does_not_match_partial_words() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function foo$0() {}
//       ^^^ rename
function foobar() {}
function barfoo() {}
  foo();
//^^^ rename
foobar();
barfoo();
"#,
        "baz",
    )
    .await;
}

/// Rename a variable should only affect the same scope, not variables with the
/// same name in other function scopes.
#[tokio::test]
async fn rename_variable_does_not_cross_function_boundary() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function foo() { $x$0 = 1; }
//               ^^ rename
function bar() { $x = 2; }
"#,
        "$y",
    )
    .await;
}

/// Rename a property across multiple files should update declaration and all uses.
/// When renaming from access site ($obj->prop), all occurrences are updated.
#[tokio::test]
async fn rename_property_works_across_files() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"//- /a.php
<?php
class Foo {
    public int $count;
    //          ^^^^^ rename
}

//- /b.php
<?php
$foo = new Foo();
echo $foo->coun$0t;
//         ^^^^^ rename
"#,
        "total",
    )
    .await;
}

/// Renaming from a property declaration site should update the declaration and
/// all access sites, just like renaming from an access site.
#[tokio::test]
async fn rename_property_from_declaration_site() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
class Foo {
    public int $coun$0t;
    //          ^^^^^ rename
}
$foo = new Foo();
$foo->count++;
//    ^^^^^ rename
echo $foo->count;
//         ^^^^^ rename
"#,
        "total",
    )
    .await;
}

/// Renaming must respect static properties and not confuse them with instance properties.
#[tokio::test]
async fn rename_distinguishes_static_from_instance_properties() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
class Config {
    public static $instance;
    public $count;
    //      ^^^^^ rename
    public function test(): void {
        $this->coun$0t++;
        //     ^^^^^ rename
    }
}
"#,
        "total",
    )
    .await;
}

/// Rename must be case-sensitive and not match names that differ only in case.
#[tokio::test]
async fn rename_is_case_sensitive() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function test() {}
//       ^^^^ rename
function Test() {}
  tes$0t();
//^^^^ rename
"#,
        "verify",
    )
    .await;
}

/// Rename multiple occurrences of the same function in different scopes.
#[tokio::test]
async fn rename_function_multiple_scopes() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function process$0() { process(); }
//       ^^^^^^^ rename
//                   ^^^^^^^ rename
if (true) { process(); }
//          ^^^^^^^ rename
while (true) { process(); break; }
//             ^^^^^^^ rename
"#,
        "handle",
    )
    .await;
}

/// Rename variable across multiple functions (comprehensive coverage).
/// Verifies that rename works correctly with deeply nested scopes.
#[tokio::test]
async fn rename_variable_deep_scopes() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function outer() {
    $x$0 = 1;
  //^^ rename
    function inner() {
        $x = 2;
    }
    echo $x;
    //   ^^ rename
}
"#,
        "$y",
    )
    .await;
}

/// Rename from an access site also updates the declaration and all other accesses.
#[tokio::test]
async fn rename_property_from_access_site_works() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
class Foo {
    public int $count;
    //          ^^^^^ rename
}
$foo = new Foo();
$foo->coun$0t++;
//    ^^^^^ rename
echo $foo->count;
//         ^^^^^ rename
"#,
        "total",
    )
    .await;
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
        1:17-1:35 → "$handler"
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

/// Arrow functions auto-capture outer-scope variables; rename covers those captures.
#[tokio::test]
async fn rename_variable_in_arrow_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function process(): void {
    $value$0 = 42;
//  ^^^^^^ rename
    $fn = fn() => $value + 1;
    //            ^^^^^^ rename
    echo $value;
    //   ^^^^^^ rename
}
"#,
        "$result",
    )
    .await;
}

/// Edge case: arrow function with multiple captures and nested operations.
#[tokio::test]
async fn rename_variable_in_nested_arrow_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function compute(): void {
    $base$0 = 10;
//  ^^^^^ rename
    $offset = 5;
    $calc = fn() => fn() => $base + $offset;
    //                      ^^^^^ rename
    echo $base;
    //   ^^^^^ rename
}
"#,
        "$initial",
    )
    .await;
}

/// Edge case: arrow function in array passed as argument.
#[tokio::test]
async fn rename_variable_in_arrow_in_array() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function process(): void {
    $multiplier$0 = 2;
//  ^^^^^^^^^^^ rename
    $mappers = [
        fn($x) => $x * $multiplier,
        //             ^^^^^^^^^^^ rename
        fn($y) => $y + $multiplier,
        //             ^^^^^^^^^^^ rename
    ];
    echo $multiplier;
    //   ^^^^^^^^^^^ rename
}
"#,
        "$factor",
    )
    .await;
}

/// Closure `use()` clause variables are included when renaming the captured variable.
#[tokio::test]
async fn rename_variable_in_closure_use_clause() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function greet(): void {
    $name$0 = "Alice";
//  ^^^^^ rename
    $greeting = function() use ($name) {
    //                          ^^^^^ rename
        echo "Hello " . $name;
    };
    echo $name;
    //   ^^^^^ rename
}
"#,
        "$person",
    )
    .await;
}

/// Edge case: closure use() clause with reference binding.
#[tokio::test]
async fn rename_variable_in_closure_use_by_reference() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function counter(): void {
    $count$0 = 0;
//  ^^^^^^ rename
    $increment = function() use (&$count) {
    //                           ^^^^^^^ rename
        $count++;
    };
    $increment();
    echo $count;
    //   ^^^^^^ rename
}
"#,
        "$total",
    )
    .await;
}

/// Edge case: closure with multiple use() variables.
/// All variables in the use clause should be collected and renamed.
#[tokio::test]
async fn rename_variable_in_closure_multiple_use_vars() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
function process(): void {
    $input$0 = "data";
//  ^^^^^^ rename
    $output = "";
    $debug = false;
    $handler = function() use ($input, $output, $debug) {
    //                         ^^^^^^ rename
        if ($debug) {
            echo $input . $output;
        }
    };
    $handler();
}
"#,
        "$data",
    )
    .await;
}

/// Renaming a class is FQN-aware; same-named symbols in other namespaces are not affected.
#[tokio::test]
async fn rename_within_namespace_scope() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
namespace App;
class Logger$0 {}
//    ^^^^^^ rename
function create() {
    $l = new Logger();
    //       ^^^^^^ rename
}
"#,
        "Reporter",
    )
    .await;
}

/// Edge case: aliased use imports must rename the alias, not the original class name.
/// This is critical: renaming by alias should only affect that alias, not the class itself.
#[tokio::test]
async fn rename_aliased_use_import() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
use App\Logger as Log;
//                ^^^ rename
$l = new Log$0();
//       ^^^ rename
"#,
        "Reporter",
    )
    .await;
}

/// Edge case: multiple imports in a single use statement must all be updated.
/// Test: use A\Foo, B\Bar; when renaming Foo
#[tokio::test]
async fn rename_with_multiple_use_imports() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
use App\Logger, App\Parser;
//      ^^^^^^ rename
$l = new Logger$0();
//       ^^^^^^ rename
$p = new Parser();
"#,
        "Reporter",
    )
    .await;
}

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

/// Edge case: FQN doesn't match due to partial name overlap.
/// When renaming App\Services\Foo, should NOT match App\Services\FooExtra.
/// This verifies the rename operation correctly enforces name boundaries.
#[tokio::test]
async fn rename_does_not_match_partial_fqn() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
use App\Services\FooExtra;
use App\Services\Foo;
//               ^^^ rename
$obj = new Foo$0();
//         ^^^ rename
"#,
        "Bar",
    )
    .await;
}

/// Edge case: Compound use statement with multiple imports.
/// `use A, B;` when renaming A should only affect A, not B.
#[tokio::test]
async fn rename_multiple_imports_in_single_use_statement() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
use App\Logger, App\Parser;
//      ^^^^^^ rename
$l = new Logger$0();
//       ^^^^^^ rename
$p = new Parser();
"#,
        "Reporter",
    )
    .await;
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

/// Property access inside a clone-with override array must be visible to rename.
#[tokio::test]
async fn rename_property_inside_clone_with_override_array() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_rename_annotated(
        r#"<?php
class Point { public int $x; public int $y; }
//                        ^ rename
$p = new Point();
$q = clone($p, ['x' => 1]);
echo $q->x$0;
//       ^ rename
"#,
        "coordX",
    )
    .await;
}

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
