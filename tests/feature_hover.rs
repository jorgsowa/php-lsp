//! Comprehensive hover coverage.
//!
//! Each scenario is an independent `#[tokio::test]` using the multi-file
//! fixture DSL with `$0` cursor markers. Assertions check key substrings so
//! tests survive minor formatting changes while still catching regressions.

mod common;

use common::TestServer;

async fn hover(server: &mut TestServer, src: &str) -> String {
    server.check_hover(src).await
}

#[tokio::test]
async fn hover_function() {
    let mut s = TestServer::new().await;
    let v = hover(&mut s, r#"<?php function gr$0eet(): void {}"#).await;
    assert!(v.contains("greet"), "expected 'greet' in {v}");
}

#[tokio::test]
async fn hover_function_with_signature() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php function gr$0eet(string $name, int $count = 1): string {}"#,
    )
    .await;
    assert!(v.contains("greet"));
    assert!(v.contains("string"));
    assert!(v.contains("$name"));
}

#[tokio::test]
async fn hover_method() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
}"#,
    )
    .await;
    assert!(v.contains("hello"));
}

#[tokio::test]
async fn hover_static_method() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
class Registry {
    public static function ge$0t(string $k): mixed {}
}"#,
    )
    .await;
    assert!(v.contains("get"));
}

#[tokio::test]
async fn hover_class_identifier() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
class Gre$0eter {}
"#,
    )
    .await;
    assert!(v.contains("Greeter"), "expected class name in hover: {v}");
}

#[tokio::test]
async fn hover_enum_identifier() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
enum Stat$0us { case Active; case Inactive; }
"#,
    )
    .await;
    assert!(v.contains("Status"), "expected enum name in hover: {v}");
}

#[tokio::test]
async fn hover_interface_identifier() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
interface Writ$0able { public function write(): void; }
"#,
    )
    .await;
    assert!(
        v.contains("Writable"),
        "expected interface name in hover: {v}"
    );
}

#[tokio::test]
async fn hover_docblock_annotated_function() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
/**
 * Greets the user.
 * @param string $name the name
 * @return string
 */
function gr$0eet(string $name): string { return $name; }
"#,
    )
    .await;
    assert!(v.contains("greet"));
    assert!(v.contains("Greets"), "expected docblock body in hover: {v}");
    assert!(v.contains("@param"), "expected @param in hover: {v}");
}

#[tokio::test]
async fn hover_method_call_resolves_receiver_class() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
class Mailer { public function process(string $to): bool {} }
class Queue  { public function process(int $id): void {} }
$mailer = new Mailer();
$mailer->pro$0cess('');
"#,
    )
    .await;
    assert!(v.contains("Mailer"), "expected 'Mailer' in {v}");
    assert!(
        !v.contains("int $id"),
        "must not leak Queue::process params: {v}"
    );
}

#[tokio::test]
async fn hover_variable_is_scoped_to_method() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
class Widget {}
class Invoice {}
class Service {
    public function a(): void { $result = new Widget(); }
    public function b(): void { $res$0ult = new Invoice(); }
}
"#,
    )
    .await;
    assert!(!v.contains("Widget"));
    assert!(v.contains("Invoice"));
}

#[tokio::test]
async fn hover_missing_symbol_returns_nothing() {
    let mut s = TestServer::new().await;
    let v = hover(&mut s, r#"<?php fo$0o();"#).await;
    assert_eq!(v, "<no hover>", "expected no hover for undefined symbol");
}

#[tokio::test]
async fn hover_across_files_via_use() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"//- /src/Greeter.php
<?php
namespace App;
class Greeter {
    public function hello(): string { return 'hi'; }
}

//- /src/main.php
<?php
use App\Greeter;
$g = new Greeter();
$g->hel$0lo();
"#,
    )
    .await;
    assert!(v.contains("hello"));
}

#[tokio::test]
async fn hover_property_access() {
    let mut s = TestServer::new().await;
    let v = hover(
        &mut s,
        r#"<?php
class User {
    public string $name = '';
}
$u = new User();
echo $u->na$0me;
"#,
    )
    .await;
    assert!(v.contains("$name"), "expected property name in hover: {v}");
    assert!(v.contains("string"), "expected property type in hover: {v}");
}
