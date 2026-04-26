//! Comprehensive go-to-definition / declaration / typeDefinition coverage.

mod common;

use common::TestServer;
use expect_test::expect;

#[tokio::test]
async fn definition_function_same_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
function greet(): void {}
//       ^^^^^ def
gr$0eet();
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_method_call_same_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Greeter {
    public function hello(): string { return 'hi'; }
    //              ^^^^^ def
}
$g = new Greeter();
$g->hel$0lo();
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_static_method() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Reg {
    public static function get(): void {}
    //                     ^^^ def
}
Reg::g$0et();
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_cross_file_via_psr4() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"//- /src/Greeter.php
<?php
namespace App;
class Greeter {
    public function hello(): string { return 'hi'; }
    //              ^^^^^ def
}

//- /src/main.php
<?php
use App\Greeter;
$g = new Greeter();
$g->hel$0lo();
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_class_in_new() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Widget {}
//    ^^^^^^ def
$w = new Wid$0get();
"#,
    )
    .await;
}

/// Cross-file goto-definition for a namespace-free class — exercises the
/// `find_in_indexes` path where the defining file is opened but not the
/// active file.
#[tokio::test]
async fn definition_cross_file_simple_class() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"//- /greeter.php
<?php
class Greeter {}
//    ^^^^^^^ def

//- /user.php
<?php
$g = new Gr$0eeter();
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_returns_none_for_missing_symbol() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
no$0thing_here();
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn declaration_on_interface_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
interface Writable { public function write(): void; }
class F implements Writable { public function write(): void {} }
$f = new F();
$f->wr$0ite();
"#,
        )
        .await;
    expect!["main.php:1:37-1:42"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_on_variable() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
class User {}
$u = new User();
$$0u;
"#,
        )
        .await;
    expect!["main.php:1:6-1:10"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_on_interface() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Writ$0able { public function write(): void; }
class A implements Writable { public function write(): void {} }
class B implements Writable { public function write(): void {} }
"#,
        )
        .await;
    expect![[r#"
        main.php:2:6-2:7
        main.php:3:6-3:7"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn definition_trait_use_resolves_to_trait_decl() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
trait Greeting {
//    ^^^^^^^^ def
    public function sayHello(string $name): string { return ""; }
}
class Greeter {
    use $0Greeting;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_trait_method_via_this() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
trait Greeting {
    public function sayHello(string $name): string {
    //              ^^^^^^^^ def
        return "";
    }
}
class Greeter {
    use Greeting;
    public function run(): string { return $this->$0sayHello('world'); }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_on_unknown_symbol_returns_null() {
    let mut s = TestServer::new().await;
    s.open("unk.php", "<?php\n$x = new UnknownClass();\n").await;
    let resp = s.definition("unk.php", 1, 13).await;
    assert!(resp["error"].is_null(), "definition errored: {resp:?}");
    let result = &resp["result"];
    let is_empty = result.is_null() || result.as_array().map(|a| a.is_empty()).unwrap_or(false);
    assert!(
        is_empty,
        "unknown symbol should have no definition, got: {result:?}"
    );
}
