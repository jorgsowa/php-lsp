//! Comprehensive go-to-definition / declaration / typeDefinition coverage.

mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::json;

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
async fn definition_interface_method_same_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
interface Serializable {
    public function seri$0alize(): string;
    //              ^^^^^^^^^ def
}
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_interface_constant_same_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
interface Limits {
    const MA$0X_SIZE = 100;
    //    ^^^^^^^^ def
}
"#,
    )
    .await;
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

// --- cross-file definition (psr4-mini fixture) ---

async fn psr4_bring_up() -> TestServer {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;
    server
}

async fn psr4_open(server: &mut TestServer, path: &str) {
    let (text, _, _) = server.locate(path, "<?php", 0);
    server.open(path, &text).await;
}

/// Goto-definition on a `use`-imported class type hint must jump across files.
/// `User $user` in Greeter::greet resolves to `class User` in Model/User.php.
#[tokio::test]
async fn goto_definition_resolves_use_import_across_files() {
    let mut server = psr4_bring_up().await;
    psr4_open(&mut server, "src/Service/Greeter.php").await;
    let (_, line, ch) = server.locate("src/Service/Greeter.php", "User $user", 0);

    let resp = server.definition("src/Service/Greeter.php", line, ch).await;
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected cross-file definition: {resp:?}"
    );
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    let uri = loc["uri"].as_str().unwrap();
    assert!(
        uri.ends_with("src/Model/User.php"),
        "definition must resolve to User.php, got: {uri}"
    );
    // `class User` is on line 4 (0-indexed); the server returns a line-start range.
    assert_eq!(
        loc["range"]["start"]["line"],
        json!(4),
        "wrong line: {loc:?}"
    );
}

/// Goto-definition on a method call across files: `$user->greeting()` in
/// Greeter must jump to `User::greeting` in Model/User.php (line 12, char 20).
#[tokio::test]
async fn goto_definition_method_call_across_files() {
    let mut server = psr4_bring_up().await;
    psr4_open(&mut server, "src/Service/Greeter.php").await;
    let (_, line, ch) = server.locate("src/Service/Greeter.php", "greeting()", 0);

    let resp = server.definition("src/Service/Greeter.php", line, ch).await;
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected cross-file method definition: {resp:?}"
    );
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    assert!(
        loc["uri"].as_str().unwrap().ends_with("src/Model/User.php"),
        "method definition must land in User.php, got: {loc:?}"
    );
    // `public function greeting()` is on line 12; the server returns a line-start range.
    assert_eq!(
        loc["range"]["start"]["line"],
        json!(12),
        "wrong line: {loc:?}"
    );
}

/// go-to-definition on a promoted constructor property should jump to the
/// parameter declaration, not to an unrelated class that happens to have a
/// property with the same name.
#[tokio::test]
async fn definition_promoted_property_same_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Service {
    public function __construct(private object $repo) {}
    //                                          ^^^^ def
    public function run(): void { $this->re$0po; }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_promoted_property_not_hijacked_by_other_class() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"//- /service.php
<?php
class Service {
    public function __construct(private object $repo) {}
    //                                          ^^^^ def
    public function run(): void { $this->re$0po; }
}

//- /other.php
<?php
class Other {
    public object $repo;
}
"#,
    )
    .await;
}

/// Cursor on `$repo` inside the constructor body itself (as a parameter
/// variable, not a property access) should resolve to the promoted param decl.
#[tokio::test]
async fn definition_promoted_property_cursor_in_constructor_body() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Builder {
    public function __construct(private string $name) {
    //                                          ^^^^ def
        echo $na$0me;
    }
}
"#,
    )
    .await;
}

/// Untyped promoted param with only a `@param` docblock — the original
/// scenario the user reported where definition jumped to an unrelated class.
#[tokio::test]
async fn definition_promoted_property_docblock_typed() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"//- /service.php
<?php
class Service {
    /** @param object $repo */
    public function __construct(private $repo) {}
    //                                   ^^^^ def
    public function run(): void { $this->re$0po; }
}

//- /other.php
<?php
class Other {
    public object $repo;
}
"#,
    )
    .await;
}

/// True cross-file definition: cursor in one file, promoted param declaration
/// in a different file's constructor.
#[tokio::test]
async fn definition_promoted_property_cross_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"//- /src/Repository.php
<?php
class Repository {
    public function __construct(private object $conn) {}
    //                                          ^^^^ def
}

//- /src/main.php
<?php
$r = new Repository($db);
$r->co$0nn;
"#,
    )
    .await;
}
