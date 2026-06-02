//! Cursor detection helper tests (protocol-wired).
//! Tests for find_use_insert_line, is_after_arrow, and cursor_is_on_method_decl
//! through observable LSP behavior.

use super::*;
use expect_test::expect;

// ── find_use_insert_line tests ──────────────────────────────────────────
// These test where use statements will be inserted, observable through
// completion and hover behavior that depends on correct use insertion logic.

#[tokio::test]
async fn use_insert_line_completion_after_namespace() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let completion = s
        .check_completion_ordered(
            r#"<?php
namespace App\Services;
class Servi$0
"#,
        )
        .await;
    expect!["Class       Servi"].assert_eq(&completion);
}

// ── is_after_arrow tests ────────────────────────────────────────────────
// Test property/method completion and hover after `->`

#[tokio::test]
async fn completion_after_arrow_shows_properties() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let completion = s
        .check_completion(
            r#"<?php
class MyClass {
    public $name = 'test';
}
$obj = new MyClass();
$obj->$0
"#,
        )
        .await;
    // Should offer property completion after ->
    expect!["Property    $name"].assert_eq(&completion);
}

#[tokio::test]
async fn completion_after_arrow_shows_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let completion = s
        .check_completion(
            r#"<?php
class MyClass {
    public function getValue() { return 1; }
}
$obj = new MyClass();
$obj->$0
"#,
        )
        .await;
    // Should offer method completion after ->
    expect!["Method      getValue"].assert_eq(&completion);
}

#[tokio::test]
async fn completion_without_arrow() {
    let mut s = TestServer::new().await;
    let completion = s
        .check_completion(
            r#"<?php
function greet() {}
gre$0();
"#,
        )
        .await;
    // Should offer function completion
    expect!["Function    greet"].assert_eq(&completion);
}

#[tokio::test]
async fn completion_after_arrow_at_start_of_property() {
    let mut s = TestServer::new().await;
    let completion = s
        .check_completion(
            r#"<?php
class MyClass {
    public $name;
    public function test() {
        $this->nam$0;
    }
}
"#,
        )
        .await;
    // Should complete property name after ->
    expect!["Property    $name"].assert_eq(&completion);
}

#[tokio::test]
async fn hover_after_arrow_shows_property_type() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
class MyClass {
    public string $name = 'test';
}
$obj = new MyClass();
echo $obj->nam$0e;
"#,
        )
        .await;
    // Should show property info when hovering after ->
    expect![[r#"
        ```php
        (property) public MyClass::$name: string
        ```"#]]
    .assert_eq(&hover);
}

// ── cursor_is_on_method_decl tests ──────────────────────────────────────
// Test detection of cursor on method declaration

#[tokio::test]
async fn hover_on_method_name_in_class() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
class C {
    public function ad$0d() {}
}
"#,
        )
        .await;
    // Should recognize method name
    expect![[r#"
        ```php
        public function add()
        ```"#]]
    .assert_eq(&hover);
}

#[tokio::test]
async fn free_function_declaration_not_confused_with_method() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
function ad$0d() {}
"#,
        )
        .await;
    // Should recognize function, not method
    expect![[r#"
        ```php
        function add()
        ```"#]]
    .assert_eq(&hover);
}

#[tokio::test]
async fn method_call_site_definition_resolves() {
    let mut s = TestServer::new().await;
    let def = s
        .check_definition(
            r#"<?php
class C {
    public function add() {}
}
$c = new C();
$c->ad$0d();
"#,
        )
        .await;
    // Definition on method call should resolve to method declaration
    expect!["main.php:2:20-2:23"].assert_eq(&def);
}

#[tokio::test]
async fn interface_method_declaration_detected() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
interface I {
    public function ad$0d(): void;
}
"#,
        )
        .await;
    // Should recognize interface method
    expect![[r#"
        ```php
        public function add(): void
        ```"#]]
    .assert_eq(&hover);
}

#[tokio::test]
async fn trait_method_declaration_detected() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
trait T {
    public function ad$0d() {}
}
"#,
        )
        .await;
    // Should recognize trait method
    expect![[r#"
        ```php
        public function add()
        ```"#]]
    .assert_eq(&hover);
}

#[tokio::test]
async fn enum_method_declaration_detected() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
enum Status {
    public function lab$0el(): string { return 'x'; }
}
"#,
        )
        .await;
    // Should recognize enum method
    expect![[r#"
        ```php
        public function label(): string
        ```"#]]
    .assert_eq(&hover);
}

#[tokio::test]
async fn method_in_unbraced_namespace_detected() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
namespace App;
class C {
    public function ad$0d() {}
}
"#,
        )
        .await;
    // Method in unbraced namespace should be detected
    expect![[r#"
        ```php
        public function add()
        ```"#]]
    .assert_eq(&hover);
}

#[tokio::test]
async fn method_in_braced_namespace_detected() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
namespace App {
    class C {
        public function ad$0d() {}
    }
}
"#,
        )
        .await;
    // Method in braced namespace should be detected
    expect![[r#"
        ```php
        public function add()
        ```"#]]
    .assert_eq(&hover);
}

#[tokio::test]
async fn definition_request_on_method_resolves_correctly() {
    let mut s = TestServer::new().await;
    let def = s
        .check_definition(
            r#"<?php
class C {
    public function ad$0d() {}
}
"#,
        )
        .await;
    // Definition on method should resolve to the method itself
    expect!["main.php:2:20-2:23"].assert_eq(&def);
}
