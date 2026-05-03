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
async fn unknown_word_in_namespace_does_not_inherit_namespace_prefix() {
    // PHP's resolver falls back to global for unqualified function calls,
    // and for classes the FQCN can't be inferred without explicit
    // qualification. The moniker resolver therefore returns the bare word
    // when no local declaration and no `use` import match.
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nnamespace App;\nclass Foo {}\n$x = some$0Helper();\n")
        .await;
    expect!["php:someHelper kind=export unique=project"].assert_eq(&out);
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

// ── member-name declaration sites ───────────────────────────────────────────
// Cursor on a method/property/class-const/enum-case name produces
// `Class::name` (or `Ns\\Class::name`, `Ns\\Class::$prop`).

#[tokio::test]
async fn class_method_declaration_uses_class_member_form() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nclass Foo {\n    public function ba$0r(): void {}\n}\n")
        .await;
    expect!["php:Foo::bar kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn class_method_in_namespace_qualifies_with_class_fqcn() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker(
            "<?php\nnamespace App;\nclass Foo {\n    public function ba$0r(): void {}\n}\n",
        )
        .await;
    expect!["php:App\\Foo::bar kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn class_property_uses_dollar_prefix() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nclass Foo {\n    public int $cou$0nter = 0;\n}\n")
        .await;
    expect!["php:Foo::$counter kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn class_const_uses_class_member_form() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nclass Foo {\n    const VER$0SION = '1';\n}\n")
        .await;
    expect!["php:Foo::VERSION kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn interface_method_qualifies_with_interface_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\ninterface Greeter {\n    public function gree$0t(): string;\n}\n")
        .await;
    expect!["php:Greeter::greet kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn trait_method_qualifies_with_trait_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\ntrait Greets {\n    public function h$0i(): void {}\n}\n")
        .await;
    expect!["php:Greets::hi kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn enum_case_qualifies_with_enum_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nenum Color {\n    case Re$0d;\n}\n")
        .await;
    expect!["php:Color::Red kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn enum_method_qualifies_with_enum_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker(
            "<?php\nenum Color {\n    case Red;\n    public function la$0bel(): string { return 'r'; }\n}\n",
        )
        .await;
    expect!["php:Color::label kind=export unique=project"].assert_eq(&out);
}

#[tokio::test]
async fn enum_case_in_namespace_qualifies_with_fqcn() {
    let mut s = TestServer::new().await;
    let out = s
        .check_moniker("<?php\nnamespace App;\nenum Color {\n    case Re$0d;\n}\n")
        .await;
    expect!["php:App\\Color::Red kind=export unique=project"].assert_eq(&out);
}
