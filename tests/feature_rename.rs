//! Rename coverage: prepareRename bounds + actual rename across files.

mod common;

use common::TestServer;
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
