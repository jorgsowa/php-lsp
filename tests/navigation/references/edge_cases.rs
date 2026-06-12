//! Edge case and regression tests: partial matching, kind filtering, class references, attributes.

use super::*;

#[tokio::test]
async fn references_no_partial_name_match() {
    // `greet` must not include occurrences of `greeting`.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function gr$0eet(): void {}
//       ^^^^^ def
function greeting(): void {}
  greet();
//^^^^^ ref
greeting();
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_includes_type_hints_and_extends() {
    // When cursor is on a class name (not __construct), refs include structural
    // usages: type hints, `extends`, and `instanceof`. No `new Ev$0ent()` is
    // present so the codebase fast path (which only tracks instantiation sites)
    // falls back to the AST walker that catches all class references.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Ev$0ent {}
//    ^^^^^ def
class UserEvent extends Event {}
//                      ^^^^^ ref
function dispatch(Event $e): void {}
//                ^^^^^ ref
$e = null;
if ($e instanceof Event) {}
//                ^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_type_hint_with_new_call() {
    // When a class appears both as a type hint AND in a new expression, find-references
    // must include ALL sites — not just the new call. This is the regression case where
    // the salsa fast path returned only `new Widget()` and silently dropped type hints.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Wi$0dget {}
//    ^^^^^^ def
function foo(Widget $w): Widget {}
//           ^^^^^^ ref
//                       ^^^^^^ ref
$x = new Widget();
//       ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_used_as_attribute() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Ro$0ute {}
//    ^^^^^ def
#[Route]
//^^^^^ ref
class HomeController {}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_as_anonymous_class_base() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Ba$0se {}
//    ^^^^ def
$x = new class extends Base {};
//                     ^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_in_closure_type_hints() {
    // Class names in closure parameter and return type hints must appear in
    // find-references results — mir v0.38 added ClassReference tracking for
    // closure/arrow-function type positions; the AST walker covers them too.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Pay$0load {}
//    ^^^^^^^ def
$handler = function(Payload $p): Payload { return $p; };
//                  ^^^^^^^ ref
//                               ^^^^^^^ ref
$mapper = fn(Payload $x): Payload => $x;
//           ^^^^^^^ ref
//                        ^^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_method_excludes_cross_file_free_function() {
    // Method refs on C::add must not include the free-function `add()`.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"//- /a.php
<?php
class C {
    public function a$0dd() {}
    //              ^^^ def
}

//- /b.php
<?php
function add() {}
add();
$c->add();
//  ^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_class_in_property_default() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class Sta$0tus {
//    ^^^^^^ def
    const ACTIVE = 1;
}
class Foo {
    public int $state = Status::ACTIVE;
    //                  ^^^^^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_static_method_call_in_class_property_default() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class C {
    public int $x = C::ma$0ke();
    //                 ^^^^ ref
    public static function make(): int {
    //                     ^^^^ def
        return 0;
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_static_method_call_in_trait_property_default() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
trait T {
    public int $x = self::in$0it();
    //                    ^^^^ ref
    public static function init(): int {
    //                     ^^^^ def
        return 0;
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_function_decl_excludes_method_with_same_name() {
    // Symmetric to references_on_method_decl_returns_method_refs_not_function_refs:
    // cursor on free-function declaration — method decl and method call must be excluded.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function a$0dd(): void {}
//       ^^^ def
  add();
//^^^ ref
class C { public function add(): void {} }
$c->add();
"#,
    )
    .await;
}

#[tokio::test]
async fn references_function_call_inside_enum_method() {
    // A free-function call inside an enum method body must be found by references.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function hel$0per(): void {}
//       ^^^^^^ def
enum Status {
    public function label(): string { return helper(); }
    //                                       ^^^^^^ ref
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_function_decl_excludes_interface_method() {
    // kind=Function must not return the interface method declaration.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
function a$0dd(): void {}
//       ^^^ def
  add();
//^^^ ref
interface I {
    public function add(): void;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_interface_method_excluded_with_include_declaration_false() {
    // With includeDeclaration=false the interface method declaration must not appear.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let opened = s
        .open_fixture(
            r#"<?php
interface I {
    public function a$0dd(): void;
}
$obj->add();
"#,
        )
        .await;
    let c = opened.cursor();

    let resp = s.references(&c.path, c.line, c.character, false).await;
    assert!(resp["error"].is_null(), "references error: {resp:?}");
    let lines: Vec<u32> = resp["result"]
        .as_array()
        .expect("expected array")
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect();
    assert!(
        !lines.contains(&2),
        "interface method declaration (line 2) must be excluded with includeDeclaration=false: {lines:?}"
    );
    assert!(
        lines.contains(&4),
        "call site (line 4) must be present: {lines:?}"
    );
}

#[tokio::test]
async fn references_method_refs_only_when_class_and_method_share_name() {
    // Edge case: a class and one of its methods share the same name.
    // Cursor on the method call — only the method declaration and call should appear;
    // the class declaration must not be returned as an extra reference.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_references_annotated(
        r#"<?php
class get {
    public function get(): void {}
    //              ^^^ def
}
$obj->ge$0t();
//    ^^^ ref
"#,
    )
    .await;
}
