//! Rename coverage: prepareRename bounds + actual rename across files.

mod common;

use common::{TestServer, canonicalize_workspace_edit};
use expect_test::expect;

#[tokio::test]
async fn prepare_rename_on_identifier() {
    let mut s = TestServer::new().await;
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
    let out = s
        .check_rename(
            r#"<?php
function gre$0et(): void {}
greet();
greet();
"#,
            "salute",
        )
        .await;
    expect![[r#"
        // main.php
        1:9-1:14 → "salute"
        2:0-2:5 → "salute"
        3:0-3:5 → "salute""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn rename_method_across_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
}
$g = new Greeter();
$g->hello();
"#,
            "salute",
        )
        .await;
    expect![[r#"
        // main.php
        2:20-2:25 → "salute"
        5:4-5:9 → "salute""#]]
    .assert_eq(&out);
}

/// Regression: renaming a variable inside an enum method previously produced
/// zero edits because collect_in_fn_at had no arm for StmtKind::Enum.
#[tokio::test]
async fn rename_variable_inside_enum_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
enum Status {
    public function label($a$0rg) { return $arg + 1; }
}
"#,
            "value",
        )
        .await;
    expect![[r#"
        // main.php
        2:26-2:30 → "$value"
        2:41-2:45 → "$value""#]]
    .assert_eq(&out);
}

/// Regression: renaming a variable parameter in an interface method previously
/// produced zero edits because collect_in_fn_at gated param collection inside
/// `if let Some(body)`, but interface methods have no body.
#[tokio::test]
async fn rename_variable_interface_method_param() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
interface Logger {
    public function log($mes$0sage): void;
}
"#,
            "$msg",
        )
        .await;
    expect![[r#"
        // main.php
        2:24-2:32 → "$msg""#]]
    .assert_eq(&out);
}

/// Regression: same bug as above but for abstract class methods.
#[tokio::test]
async fn rename_variable_abstract_class_method_param() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
abstract class Processor {
    abstract public function process($in$0put): string;
}
"#,
            "$data",
        )
        .await;
    expect![[r#"
        // main.php
        2:37-2:43 → "$data""#]]
    .assert_eq(&out);
}

/// Regression: same bug as above but for abstract trait methods.
#[tokio::test]
async fn rename_variable_abstract_trait_method_param() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
trait Formattable {
    abstract public function format($da$0ta): string;
}
"#,
            "$input",
        )
        .await;
    expect![[r#"
        // main.php
        2:36-2:41 → "$input""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn rename_class_updates_new_sites() {
    let mut s = TestServer::new().await;
    let out = s
        .check_rename(
            r#"<?php
class Wid$0get {}
$a = new Widget();
$b = new Widget();
"#,
            "Gadget",
        )
        .await;
    expect![[r#"
        // main.php
        1:6-1:12 → "Gadget"
        2:9-2:15 → "Gadget"
        3:9-3:15 → "Gadget""#]]
    .assert_eq(&out);
}

/// `prepareRename` on a PHP keyword must return null so the editor greys out
/// the rename action rather than presenting an empty rename dialog.
#[tokio::test]
async fn prepare_rename_on_keyword_returns_nothing() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_rename(
            r#"<?php
func$0tion greet(): void {}
"#,
        )
        .await;
    expect!["<not renameable>"].assert_eq(&out);
}

/// `prepareRename` on a variable should return the range covering the
/// variable name (without `$`) so editors highlight the right text.
#[tokio::test]
async fn prepare_rename_on_variable() {
    let mut s = TestServer::new().await;
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
    let out = s
        .check_rename(
            r#"<?php
class Counter {
    public int $count = 0;
    public function inc(): void { $this->coun$0t++; }
    public function get(): int  { return $this->count; }
}
"#,
            "total",
        )
        .await;
    expect![[r#"
        // main.php
        2:16-2:21 → "total"
        3:41-3:46 → "total"
        4:48-4:53 → "total""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn rename_on_nonexistent_symbol_does_not_error() {
    let mut s = TestServer::new().await;
    s.open("rn.php", "<?php\n// nothing to rename\n").await;
    let resp = s.rename("rn.php", 1, 5, "NewName").await;
    assert!(resp["error"].is_null(), "rename errored: {resp:?}");
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

        // src/Service/Registry.php
        4:14-4:18 → "Account""#]]
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
    let changes = resp["result"]["changes"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    assert!(
        changes.is_empty(),
        "rename-to-self must not produce edits, got: {changes:?}"
    );
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
