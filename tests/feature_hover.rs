//! Comprehensive hover coverage.

mod common;

use common::{TestServer, render_hover};
use expect_test::expect;

#[tokio::test]
async fn hover_function() {
    let mut s = TestServer::new().await;
    let v = s.check_hover(r#"<?php function gr$0eet(): void {}"#).await;
    expect![[r#"
        ```php
        function greet(): void
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_function_with_signature() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(r#"<?php function gr$0eet(string $name, int $count = 1): string {}"#)
        .await;
    expect![[r#"
        ```php
        function greet(string $name, int $count = 1): string
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_method() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Greeter {
    public function he$0llo(): string { return 'hi'; }
}"#,
        )
        .await;
    expect![[r#"
        ```php
        function hello(): string
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_static_method() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Registry {
    public static function ge$0t(string $k): mixed {}
}"#,
        )
        .await;
    expect![[r#"
        ```php
        function get(string $k): mixed
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_class_identifier() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Gre$0eter {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        class Greeter
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_enum_identifier() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
enum Stat$0us { case Active; case Inactive; }
"#,
        )
        .await;
    expect![[r#"
        ```php
        enum Status
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_interface_identifier() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
interface Writ$0able { public function write(): void; }
"#,
        )
        .await;
    expect![[r#"
        ```php
        interface Writable
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_docblock_annotated_function() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
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
    expect![[r#"
        ```php
        function greet(string $name): string
        ```

        ---

        Greets the user.

        **@return** `string`
        **@param** `string` `$name` — the name"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_method_call_resolves_receiver_class() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Mailer { public function process(string $to): bool {} }
class Queue  { public function process(int $id): void {} }
$mailer = new Mailer();
$mailer->pro$0cess('');
"#,
        )
        .await;
    expect![[r#"
        ```php
        Mailer::process(string $to): bool
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_variable_is_scoped_to_method() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
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
    expect!["`$result` `Invoice`"].assert_eq(&v);
}

#[tokio::test]
async fn hover_missing_symbol_returns_nothing() {
    let mut s = TestServer::new().await;
    let v = s.check_hover(r#"<?php fo$0o();"#).await;
    expect!["<no hover>"].assert_eq(&v);
}

#[tokio::test]
async fn hover_class_in_extends_clause_cross_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Base.php"), "<?php\nclass Base {}\n").unwrap();
    let child_src = "<?php\nclass Child extends Base {}\n";
    std::fs::write(tmp.path().join("Child.php"), child_src).unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    // Only open Child.php — Base.php is indexed but never opened.
    s.open("Child.php", child_src).await;
    let (_, line, col) = s.locate("Child.php", "Base", 0);
    let resp = s.hover("Child.php", line, col).await;
    expect![[r#"
        ```php
        class Base
        ```"#]]
    .assert_eq(&render_hover(&resp));
}

#[tokio::test]
async fn hover_class_as_param_type_cross_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Post.php"),
        "<?php\nclass Post { public string $title = ''; }\n",
    )
    .unwrap();
    let ctrl_src = "<?php\nfunction show(Post $post): void {}\n";
    std::fs::write(tmp.path().join("Controller.php"), ctrl_src).unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    // Only open Controller.php — Post.php is indexed but never opened.
    s.open("Controller.php", ctrl_src).await;
    let (_, line, col) = s.locate("Controller.php", "Post", 0);
    let resp = s.hover("Controller.php", line, col).await;
    expect![[r#"
        ```php
        class Post
        ```"#]]
    .assert_eq(&render_hover(&resp));
}

#[tokio::test]
async fn hover_across_files_via_use() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
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
    expect![[r#"
        ```php
        Greeter::hello(): string
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_property_access() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class User {
    public string $name = '';
}
$u = new User();
echo $u->na$0me;
"#,
        )
        .await;
    expect![[r#"
        ```php
        (property) User::$name: string
        ```"#]]
    .assert_eq(&v);
}

/// Hovering on an enum *case* (not the enum name) should return the qualified
/// case label. If the server only indexes enum names but not individual cases
/// this will produce `<no hover>` — that is the bug to fix.
#[tokio::test]
async fn hover_enum_case_declaration() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
enum Status { case Acti$0ve; case Inactive; }
"#,
        )
        .await;
    expect![[r#"
        ```php
        case Status::Active
        ```"#]]
    .assert_eq(&v);
}

/// Hovering on a class constant must show the constant with its inferred or
/// declared type. An unimplemented constant-hover returns `<no hover>`.
#[tokio::test]
async fn hover_class_constant() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Config {
    const VERSI$0ON = 42;
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        const int VERSION = 42
        ```"#]]
    .assert_eq(&v);
}

/// A function with a nullable param type `?T` must render the `?` in hover so
/// callers can see the type is optional. Cursor is on the function name.
#[tokio::test]
async fn hover_nullable_param_type() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
function sho$0w(?string $label): void {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        function show(?string $label): void
        ```"#]]
    .assert_eq(&v);
}

/// Hovering on a trait identifier must render as `trait Name`, not `class`.
#[tokio::test]
async fn hover_trait_identifier() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
trait Logg$0able { public function log(): void {} }
"#,
        )
        .await;
    expect![[r#"
        ```php
        trait Loggable
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_trait_inherited_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
trait Greeting {
    public function sayHello(string $name): string {
        return "Hello, {$name}";
    }
}
class Greeter {
    use Greeting;
    public function run(): string {
        return $this->$0sayHello('world');
    }
}
"#,
        )
        .await;
    assert!(
        out.contains("sayHello"),
        "hover on trait-inherited method must return its signature, got: {out}"
    );
}

#[tokio::test]
async fn hover_multi_trait_alpha() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
trait A { public function alpha(): int { return 1; } }
trait B { public function beta(): int { return 2; } }
class Both {
    use A;
    use B;
    public function run(): int { return $this->$0alpha() + $this->beta(); }
}
"#,
        )
        .await;
    assert!(
        out.contains("alpha"),
        "hover on alpha() via multi-trait must mention it, got: {out}"
    );
}

#[tokio::test]
async fn hover_multi_trait_beta() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
trait A { public function alpha(): int { return 1; } }
trait B { public function beta(): int { return 2; } }
class Both {
    use A;
    use B;
    public function run(): int { return $this->alpha() + $this->$0beta(); }
}
"#,
        )
        .await;
    assert!(
        out.contains("beta"),
        "hover on beta() via multi-trait must mention it, got: {out}"
    );
}

#[tokio::test]
async fn hover_on_empty_file_returns_null_not_error() {
    let mut s = TestServer::new().await;
    s.open("empty.php", "").await;
    let resp = s.hover("empty.php", 0, 0).await;
    assert!(
        resp["error"].is_null(),
        "hover errored on empty file: {resp:?}"
    );
    assert!(
        resp["result"].is_null(),
        "hover on empty file should be null, got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn hover_past_eof_does_not_crash() {
    let mut s = TestServer::new().await;
    s.open("short.php", "<?php\nfunction f(): void {}\n").await;
    let resp = s.hover("short.php", 500, 500).await;
    assert!(resp["error"].is_null(), "hover past EOF errored: {resp:?}");
}
