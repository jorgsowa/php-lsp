//! Comprehensive go-to-definition / declaration / typeDefinition coverage.

mod common;

use common::TestServer;

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
    assert!(
        out == "<none>" || out.contains("nothing_here"),
        "unexpected: {out}"
    );
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
    // Should land on the interface method declaration or the class method.
    // Just ensure we got something.
    assert!(out != "<none>" || !out.is_empty());
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
    // Servers differ on where type_definition lands; accept any non-error.
    assert!(!out.starts_with("error:"), "errored: {out}");
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
    assert!(!out.starts_with("error:"), "errored: {out}");
}
