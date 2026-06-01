//! Comprehensive hover coverage.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn hover_backed_enum_case_in_match_arm() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Priority: int { case Low = 1; case High = 2; }
match ($p) {
    Priority::H$0igh => echo 'urgent',
}
"#,
        expect![[r#"
            ```php
            case Priority::High = 2
            ```"#]],
    )
    .await;
}

/// Confirm that static method hover in match arm still works (regression check).
#[tokio::test]
async fn hover_backed_enum_shows_backing_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Stat$0us: string { case Active = 'active'; }
"#,
        expect![[r#"
            ```php
            enum Status: string
            ```"#]],
    )
    .await;
}

/// Backed int enum.
#[tokio::test]
async fn hover_class_constant() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Config {
    const VERSI$0ON = 42;
}
"#,
        expect![[r#"
            ```php
            const int VERSION = 42
            ```"#]],
    )
    .await;
}

/// A function with a nullable param type `?T` must render the `?` in hover so
/// callers can see the type is optional. Cursor is on the function name.
#[tokio::test]
async fn hover_enum_case_declaration() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Status { case Acti$0ve; case Inactive; }
"#,
        expect![[r#"
            ```php
            case Status::Active
            ```"#]],
    )
    .await;
}

/// Hovering on a class constant must show the constant with its inferred or
/// declared type. An unimplemented constant-hover returns `<no hover>`.
#[tokio::test]
async fn hover_function_with_signature() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function gr$0eet(string $name, int $count = 1): string {}"#,
        expect![[r#"
            ```php
            function greet(string $name, int $count = 1): string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_nullable_param_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
function sho$0w(?string $label): void {}
"#,
        expect![[r#"
            ```php
            function show(?string $label): void
            ```"#]],
    )
    .await;
}

/// Hovering on a trait identifier must render as `trait Name`, not `class`.
#[tokio::test]
async fn hover_property_access() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class User {
    public string $name = '';
}
$u = new User();
echo $u->na$0me;
"#,
        expect![[r#"
            ```php
            (property) public User::$name: string
            ```"#]],
    )
    .await;
}

/// Hovering on an enum *case* (not the enum name) should return the qualified
/// case label. If the server only indexes enum names but not individual cases
/// this will produce `<no hover>` — that is the bug to fix.
#[tokio::test]
async fn hover_static_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Config {
    public static string $version = '1.0';
}
Config::$ver$0sion;
"#,
        expect![[r#"
            ```php
            (property) public static Config::$version: string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_template_at_call_site_shows_literal_t() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/** @template T @param T $x @return T */
function identity($x) { return $x; }
$myString = 'hello';
// Hovering on the return value assignment
$result = identi$0ty($myString);
"#,
        expect![[r#"
            ```php
            function identity($x)
            ```

            ---

            **@template** `T`"#]],
    )
    .await;
}
#[tokio::test]
async fn hover_template_param_type_in_signature() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/** @template T @param T $v @return T */
function box($v) { }
$result = box$0('hello');
"#,
        expect![[r#"
            ```php
            function box($v)
            ```

            ---

            **@template** `T`"#]],
    )
    .await;
}

/// At a call site, template T is shown literally (not substituted to string).
/// NOTE: Full template substitution (T → string) requires call-site argument
/// inference in type_map.rs, which is a larger architectural change deferred
/// to a future iteration. This test documents the current limitation.
#[tokio::test]
async fn hover_union_type_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Config {
    public string|int $setting = '';
}
$c = new Config();
echo $c->se$0tting;
"#,
        expect![[r#"
            ```php
            (property) public Config::$setting: string|int
            ```"#]],
    )
    .await;
}

/// mir-primary variable hover renders a *union* type. The legacy short-name
/// tracker collapsed unions to a single class, so this is a resolution-quality
/// gain that had no coverage. Guards the mir hover path.
#[tokio::test]
async fn hover_union_typed_variable_shows_union() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Cat {}
class Dog {}
function pet(Cat|Dog $a): void { $a$0; }
"#,
        expect![[r#"
            `$a` `Cat|Dog`"#]],
    )
    .await;
}
