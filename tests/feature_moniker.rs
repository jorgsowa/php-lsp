mod common;

use common::TestServer;
use expect_test::expect;

// ── basic shape & full moniker payload ──────────────────────────────────────

#[tokio::test]
async fn top_level_function_moniker_full_shape() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nfunction monik$0erFn(): void {}\n")
        .await;
    expect!["php:monikerFn kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn class_name_moniker() {
    let mut s = TestServer::new().await;
    let out = s.check_moniker("<?php\nclass Fo$0o {}\n").await;
    expect!["php:Foo kind=export unique=project"].assert_eq(&out);
}

// ── declarations: every top-level kind ──────────────────────────────────────

#[tokio::test]
async fn interface_name_moniker() {
    let mut s = TestServer::new().await;
    let out = s.check_moniker("<?php\ninterface Greete$0r {}\n").await;
    expect!["php:Greeter kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn trait_name_moniker() {
    let mut s = TestServer::new().await;
    let out = s.check_moniker("<?php\ntrait Greet$0s {}\n").await;
    expect!["php:Greets kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn enum_name_moniker() {
    let mut s = TestServer::new().await;
    let out = s.check_moniker("<?php\nenum Col$0or {}\n").await;
    expect!["php:Color kind=export unique=project"].assert_eq(&out);
}

// ── namespace handling ──────────────────────────────────────────────────────

#[tokio::test]
async fn braced_namespace_class_moniker() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nnamespace App\\Services {\n    class FooSer$0vice {}\n}\n")
        .await;
    expect!["php:App\\Services\\FooService kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn simple_namespace_class_moniker() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nnamespace App\\Http;\nclass Reque$0st {}\n")
        .await;
    expect!["php:App\\Http\\Request kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn simple_namespace_function_moniker() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nnamespace App;\nfunction hel$0per(): void {}\n")
        .await;
    expect!["php:App\\helper kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn unknown_word_in_namespace_inherits_namespace_prefix() {
    // Cursor sits on a word that has no local declaration and no `use`
    // import — for namespaced files, the resolver still attaches the file's
    // namespace prefix (PHP-style FQCN behavior).
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nnamespace App;\nclass Foo {}\n$x = some$0Helper();\n")
        .await;
    expect!["php:App\\someHelper kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn class_declared_outside_namespace_resolves_bare() {
    let mut s = TestServer::new().await;
    let out = s.check_moniker("<?php\nclass Out$0er {}\n").await;
    expect!["php:Outer kind=export unique=project"].assert_eq(&out);
}

// ── use-imports ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn use_import_resolves_reference_to_fqn() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nuse App\\Services\\Mailer;\n$m = new Mai$0ler();\n")
        .await;
    expect!["php:App\\Services\\Mailer kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn use_alias_resolves_to_fqn() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nuse App\\Http\\Request as Req;\n$r = new Re$0q();\n")
        .await;
    expect!["php:App\\Http\\Request kind=export unique=project"].assert_eq(&out);
}

// ── unqualified / fully-qualified references ───────────────────────────────

#[tokio::test]
async fn fully_qualified_reference_is_stripped_of_leading_backslash() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nclass Foo {}\n$x = new \\Fo$0o();\n")
        .await;
    expect!["php:Foo kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn unknown_bare_name_returns_word_as_identifier() {
    // No declaration, no namespace, no import — the resolver echoes the bare word.
    let mut s = TestServer::new().await;
    let out = s.check_moniker("<?php\n$x = doSome$0thing();\n").await;
    expect!["php:doSomething kind=export unique=project"].assert_eq(&out);
}

// ── positions that yield no moniker ─────────────────────────────────────────

#[tokio::test]
async fn variable_position_returns_no_moniker() {
    let mut s = TestServer::new().await;
    let out = s.check_moniker("<?php\n$fo$0o = 1;\n").await;
    expect!["<no moniker>"].assert_eq(&out);
}

#[tokio::test]
async fn whitespace_position_returns_no_moniker() {
    let mut s = TestServer::new().await;
    let out = s.check_moniker("<?php\n   $0   \nclass Foo {}\n").await;
    expect!["<no moniker>"].assert_eq(&out);
}

// ── current-behavior snapshots: positions the resolver is name-only ─────────
// `moniker_at` resolves whatever bare word is under the cursor against
// top-level declarations; it does NOT understand member context. The
// snapshots below pin the present behavior so any future improvement
// (e.g. emitting `Class::method`) shows up as a snapshot diff.

#[tokio::test]
async fn class_method_position_resolves_method_word_only() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nclass Foo {\n    public function ba$0r(): void {}\n}\n")
        .await;
    expect!["php:bar kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn class_method_in_namespace_inherits_namespace_prefix() {
    // The current resolver attaches the file's namespace prefix to ANY
    // unresolved word — including method names, which is technically wrong
    // (methods aren't independently namespaced). Pinned for visibility.
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker(
            "<?php\nnamespace App;\nclass Foo {\n    public function ba$0r(): void {}\n}\n",
        )
        .await;
    expect!["php:App\\bar kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn enum_case_position_resolves_case_word_only() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nenum Color {\n    case Re$0d;\n}\n")
        .await;
    expect!["php:Red kind=export unique=project"].assert_eq(&out);
}
