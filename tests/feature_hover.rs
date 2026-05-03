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
        public function hello(): string
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
        public static function get(string $k): mixed
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
        (property) public User::$name: string
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
    let v = s
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
    expect![[r#"
        ```php
        Greeter::sayHello(string $name): string
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_multi_trait_alpha() {
    let mut s = TestServer::new().await;
    let v = s
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
    expect![[r#"
        ```php
        Both::alpha(): int
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_multi_trait_beta() {
    let mut s = TestServer::new().await;
    let v = s
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
    expect![[r#"
        ```php
        Both::beta(): int
        ```"#]]
    .assert_eq(&v);
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
    assert!(
        resp["result"].is_null(),
        "hover past EOF should have null result, got: {resp:?}"
    );
}

// ── Backed enum ───────────────────────────────────────────────────────────────

/// `enum Status: string` must include `: string` in the hover so the caller
/// knows the backing type.
#[tokio::test]
async fn hover_backed_enum_shows_backing_type() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
enum Stat$0us: string { case Active = 'active'; }
"#,
        )
        .await;
    expect![[r#"
        ```php
        enum Status: string
        ```"#]]
    .assert_eq(&v);
}

/// Backed int enum.
#[tokio::test]
async fn hover_backed_int_enum_shows_backing_type() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
enum Priorit$0y: int { case Low = 1; case High = 2; }
"#,
        )
        .await;
    expect![[r#"
        ```php
        enum Priority: int
        ```"#]]
    .assert_eq(&v);
}

// ── Class modifiers ───────────────────────────────────────────────────────────

#[tokio::test]
async fn hover_abstract_class_shows_keyword() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
abstract class Bas$0eHandler {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        abstract class BaseHandler
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_final_class_shows_keyword() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
final class Concret$0eService {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        final class ConcreteService
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_readonly_class_shows_keyword() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
readonly class Poi$0nt { public function __construct(public float $x, public float $y) {} }
"#,
        )
        .await;
    expect![[r#"
        ```php
        readonly class Point
        ```"#]]
    .assert_eq(&v);
}

// ── Use-alias resolution ──────────────────────────────────────────────────────

/// Hovering on `Bar` where `use Foo as Bar` is in scope must show the `Foo`
/// class declaration.
#[tokio::test]
async fn hover_use_alias_resolves_to_class() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Mailer { public function send(): void {} }
use Mailer as Sender;
$s = new Send$0er();
"#,
        )
        .await;
    expect![[r#"
        ```php
        class Mailer
        ```"#]]
    .assert_eq(&v);
}

// ── Static-call disambiguation ────────────────────────────────────────────────

/// Hovering on the method name in `Worker::run()` at the call site must show
/// `Worker::run`, not `Scheduler::run`, even though both classes have `run`.
#[tokio::test]
async fn hover_static_call_resolves_correct_class() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Worker { public static function run(int $jobs): void {} }
class Scheduler { public static function run(string $cron): bool { return true; } }
Worker::ru$0n(4);
"#,
        )
        .await;
    expect![[r#"
        ```php
        Worker::run(int $jobs): void
        ```"#]]
    .assert_eq(&v);
}

/// `self::method()` at a call site resolves to the enclosing class.
#[tokio::test]
async fn hover_self_static_call_resolves_enclosing_class() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Builder {
    public static function create(): static { return new static(); }
    public function run(): void { self::crea$0te(); }
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        Builder::create(): static
        ```"#]]
    .assert_eq(&v);
}

// ── Correct receiver on multi-call line ───────────────────────────────────────

/// Two different `->process()` calls on the same line — cursor on the second
/// one must pick the second receiver, not the first.
#[tokio::test]
async fn hover_second_method_call_on_same_line_picks_correct_receiver() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class A { public function handle(string $x): bool {} }
class B { public function handle(int $n): void {} }
$a = new A(); $b = new B();
$a->handle('x'); $b->hand$0le(1);
"#,
        )
        .await;
    // Must show B::handle (int $n), not A::handle (string $x).
    expect![[r#"
        ```php
        B::handle(int $n): void
        ```"#]]
    .assert_eq(&v);
}

// ── Trait inheritance correctness ─────────────────────────────────────────────

/// Two classes each define a method named `ping`; only `Server` uses the
/// `Pingable` trait that actually has the implementation.  Hovering on
/// `$server->ping()` must show `Server::ping`, not `Client::ping`.
#[tokio::test]
async fn hover_trait_method_picks_correct_class_not_unrelated_one() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
trait Pingable { public function ping(): string { return 'pong'; } }
class Server { use Pingable; }
class Client { public function ping(): bool { return false; } }
$s = new Server();
$s->pin$0g();
"#,
        )
        .await;
    // Must show Server::ping (from trait), returning string — not Client::ping.
    expect![[r#"
        ```php
        Server::ping(): string
        ```"#]]
    .assert_eq(&v);
}

/// Hovering on a parent-class method accessed through a child instance.
#[tokio::test]
async fn hover_inherited_method_shows_child_class_name() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Animal { public function spe$0ak(): string { return '...'; } }
class Dog extends Animal {}
$d = new Dog();
$d->speak();
"#,
        )
        .await;
    // Hovering on the declaration itself — should show Animal::speak.
    expect![[r#"
        ```php
        public function speak(): string
        ```"#]]
    .assert_eq(&v);
}

/// `$dog->speak()` where Dog extends Animal must show Dog::speak (via extends
/// walk) when another class also has a method called `speak`.
#[tokio::test]
async fn hover_child_receiver_resolves_parent_method_correctly() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Animal { public function speak(): string { return '...'; } }
class Dog extends Animal {}
class Parrot { public function speak(): string { return 'hello'; } }
$d = new Dog();
$d->spea$0k();
"#,
        )
        .await;
    // Must show Dog::speak (inherited from Animal), NOT Parrot::speak.
    expect![[r#"
        ```php
        Dog::speak(): string
        ```"#]]
    .assert_eq(&v);
}

// ── Declaration-site modifiers ────────────────────────────────────────────────

#[tokio::test]
async fn hover_abstract_method_shows_modifiers() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
abstract class Base {
    abstract protected function pro$0cess(string $input): string;
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        protected abstract function process(string $input): string
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_final_method_shows_modifiers() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Locked {
    final public function sea$0l(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        public final function seal(): void
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_readonly_property_shows_modifier() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Point {
    public readonly float $x;
}
$p = new Point();
echo $p->$0x;
"#,
        )
        .await;
    expect![[r#"
        ```php
        (property) public readonly Point::$x: float
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_deprecated_function_shows_banner() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
/** @deprecated Use newGreet() instead */
function ol$0dGreet(): void {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        function oldGreet(): void
        ```

        ---

        > **Deprecated**: Use newGreet() instead"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_function_with_throws_shows_tag() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
/**
 * @throws \RuntimeException When the operation fails
 */
function ri$0sky(): void {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        function risky(): void
        ```

        ---

        **@throws** `\RuntimeException` — When the operation fails"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_static_property() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Config {
    public static string $version = '1.0';
}
Config::$ver$0sion;
"#,
        )
        .await;
    expect![[r#"
        ```php
        (property) public static Config::$version: string
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_static_property_cross_file() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"//- /caller.php
<?php
Config::$ver$0sion;

//- /Config.php
<?php
class Config {
    public static string $version = '1.0';
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        (property) public static Config::$version: string
        ```"#]]
    .assert_eq(&v);
}

// ── 1.3 First-class callable hover ──────────────────────────────────────────

#[tokio::test]
async fn hover_first_class_callable_builtin() {
    let mut s = TestServer::new().await;
    let v = s.check_hover(r#"<?php $fn = str$0len(...);"#).await;
    expect![[r#"
        ```php
        function strlen()
        ```

        [php.net documentation](https://www.php.net/function.strlen)"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_first_class_callable_user_function() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(r#"<?php function double(int $n): int {} $fn = dou$0ble(...);"#)
        .await;
    expect![[r#"
        ```php
        function double(int $n): int
        ```"#]]
    .assert_eq(&v);
}

// ── 1.1 @inheritDoc resolution ───────────────────────────────────────────────

#[tokio::test]
async fn hover_inheritdoc_shows_parent_description() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Base {
    /** Sends the payload to the remote endpoint. */
    public function send(): void {}
}
class Child extends Base {
    /** {@inheritDoc} */
    public function send(): void {}
}
$c = new Child();
$c->sen$0d();
"#,
        )
        .await;
    expect![[r#"
        ```php
        Child::send(): void
        ```

        ---

        Sends the payload to the remote endpoint."#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_inheritdoc_at_tag_form() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Base {
    /** Fetches the record. */
    public function fetch(): void {}
}
class Child extends Base {
    /** @inheritDoc */
    public function fetch(): void {}
}
$c = new Child();
$c->fet$0ch();
"#,
        )
        .await;
    expect![[r#"
        ```php
        Child::fetch(): void
        ```

        ---

        Fetches the record."#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_real_docblock_not_overwritten_by_inheritdoc() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Base {
    /** Parent description. */
    public function run(): void {}
}
class Child extends Base {
    /** Child's own description. */
    public function run(): void {}
}
$c = new Child();
$c->ru$0n();
"#,
        )
        .await;
    expect![[r#"
        ```php
        Child::run(): void
        ```

        ---

        Child's own description."#]]
    .assert_eq(&v);
}

// ── 1.2 Keyword hover ────────────────────────────────────────────────────────

#[tokio::test]
async fn hover_keyword_match() {
    let mut s = TestServer::new().await;
    let v = s.check_hover(r#"<?php $x = mat$0ch($y) {};"#).await;
    expect![["`match` — evaluates an expression against a set of arms (PHP 8.0)"]].assert_eq(&v);
}

#[tokio::test]
async fn hover_keyword_null() {
    let mut s = TestServer::new().await;
    let v = s.check_hover(r#"<?php $x = nu$0ll;"#).await;
    expect![["`null` — the null value; a variable has no value"]].assert_eq(&v);
}

#[tokio::test]
async fn hover_keyword_true() {
    let mut s = TestServer::new().await;
    let v = s.check_hover(r#"<?php $x = tr$0ue;"#).await;
    expect![["`true` — boolean true"]].assert_eq(&v);
}

#[tokio::test]
async fn hover_keyword_false() {
    let mut s = TestServer::new().await;
    let v = s.check_hover(r#"<?php $x = fal$0se;"#).await;
    expect![["`false` — boolean false"]].assert_eq(&v);
}

#[tokio::test]
async fn hover_keyword_readonly() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(r#"<?php class Foo { readon$0ly string $x; }"#)
        .await;
    expect![["`readonly` — property or class that can only be initialised once (PHP 8.1)"]]
        .assert_eq(&v);
}

#[tokio::test]
async fn hover_keyword_never() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(r#"<?php function fail(): nev$0er { throw new \Exception(); }"#)
        .await;
    expect![["`never` — return type indicating the function always throws or exits (PHP 8.1)"]]
        .assert_eq(&v);
}

#[tokio::test]
async fn hover_static_keyword_in_static_call_not_intercepted() {
    // `static::method()` — hovering `static` should NOT return the keyword doc,
    // it should fall through to the self/static class resolution.
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Base {
    public static function create(): static {}
    public static function build(): static {
        return stat$0ic::create();
    }
}
"#,
        )
        .await;
    // Should resolve to something about Base (static call), not the keyword doc.
    assert!(
        !v.contains("return type") && !v.contains("late static"),
        "static:: should not trigger keyword hover, got: {v}"
    );
}

// ── 2.4 PHP attribute hover ───────────────────────────────────────────────────

#[tokio::test]
async fn hover_attribute_class_name() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class MyAttribute {}

#[MyAttri$0bute]
class Foo {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        class MyAttribute
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_attribute_with_args() {
    // Cursor on attribute class name when the attribute has constructor arguments.
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Route {
    public function __construct(string $path) {}
}

#[Rou$0te('/api')]
class Controller {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        class Route
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_attribute_with_docblock() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
/** Marks a class as a service container. */
class Service {}

#[Servi$0ce]
class Mailer {}
"#,
        )
        .await;
    expect![[r#"
        ```php
        class Service
        ```

        ---

        Marks a class as a service container."#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_attribute_via_use_alias() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Route {}
use Route as HttpRoute;

#[HttpRou$0te]
class Api {}
"#,
        )
        .await;
    // Resolves alias → Route
    expect![[r#"
        ```php
        class Route
        ```"#]]
    .assert_eq(&v);
}

// ── 2.2 Named argument hover ──────────────────────────────────────────────────

#[tokio::test]
async fn hover_named_arg_builtin_function() {
    // PHP 8.0 named arg on a user-defined function matching a known param name.
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
function greet(string $name, int $count = 1): string { return $name; }
greet(coun$0t: 3);
"#,
        )
        .await;
    expect![[r#"
        ```php
        (parameter) int $count = 1
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_named_arg_with_docblock() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
/**
 * @param string $name The user's name.
 * @param int $age  The user's age.
 */
function register(string $name, int $age): void {}
register(na$0me: 'Alice', age: 30);
"#,
        )
        .await;
    expect![[r#"
        ```php
        (parameter) string $name
        ```

        ---

        The user's name."#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_named_arg_method_call() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Mailer {
    public function send(string $to, string $subject): bool { return true; }
}
$m = new Mailer();
$m->send(subje$0ct: 'Hello', to: 'a@b.com');
"#,
        )
        .await;
    expect![[r#"
        ```php
        (parameter) string $subject
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_named_arg_static_method() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class DB {
    public static function query(string $sql, int $limit = 100): array { return []; }
}
DB::query(lim$0it: 10);
"#,
        )
        .await;
    expect![[r#"
        ```php
        (parameter) int $limit = 100
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_named_arg_nested_call() {
    // Named arg inside a nested function call — cursor on inner call's arg.
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
function outer(string $a): string { return $a; }
function inner(int $x): int { return $x; }
outer(a: inner(x$0: 1));
"#,
        )
        .await;
    expect![[r#"
        ```php
        (parameter) int $x
        ```"#]]
    .assert_eq(&v);
}

// ── 2.3 Closure / arrow function hover ───────────────────────────────────────

#[tokio::test]
async fn hover_closure_keyword() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(r#"<?php $fn = fun$0ction(int $x, string $y): bool { return true; };"#)
        .await;
    expect![[r#"
        ```php
        function(int $x, string $y): bool
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_arrow_function_keyword() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(r#"<?php $f = f$0n(int $a): string => 'hello';"#)
        .await;
    expect![[r#"
        ```php
        fn(int $a): string
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_closure_no_params_no_return() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(r#"<?php $fn = fun$0ction() { return 1; };"#)
        .await;
    expect![[r#"
        ```php
        function()
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_closure_as_argument() {
    // Cursor on `function` keyword passed as a callback argument.
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
function apply(callable $fn): void {}
apply(fun$0ction(int $n): int { return $n * 2; });
"#,
        )
        .await;
    expect![[r#"
        ```php
        function(int $n): int
        ```"#]]
    .assert_eq(&v);
}

#[tokio::test]
async fn hover_named_function_keyword_not_intercepted() {
    // Hovering the `function` keyword in a named declaration (not a closure)
    // should not trigger the closure hover — returns nothing for the keyword itself.
    // Hover on the function *name* (not keyword) to get the signature.
    let mut s = TestServer::new().await;
    let v = s.check_hover(r#"<?php fun$0ction greet(): void {}"#).await;
    expect!["<no hover>"].assert_eq(&v);
}

#[tokio::test]
async fn hover_closure_inside_if_body() {
    // Closure nested inside an if body — the walker must recurse into if branches.
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
if (true) {
    $fn = fun$0ction(int $x): string { return (string) $x; };
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        function(int $x): string
        ```"#]]
    .assert_eq(&v);
}

/// Hovering on `new Foo()` (the constructor call) must resolve to the class definition.
#[tokio::test]
async fn hover_on_constructor_call() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Service {
    public function __construct(private string $dsn) {}
}
$svc = new Serv$0ice('db://localhost');
"#,
        )
        .await;
    expect![[r#"
        ```php
        class Service
        ```"#]]
    .assert_eq(&v);
}

/// Hovering on a property access with union type should show the union.
#[tokio::test]
async fn hover_union_type_property() {
    let mut s = TestServer::new().await;
    let v = s
        .check_hover(
            r#"<?php
class Config {
    public string|int $setting = '';
}
$c = new Config();
echo $c->se$0tting;
"#,
        )
        .await;
    expect![[r#"
        ```php
        (property) public Config::$setting: string|int
        ```"#]]
    .assert_eq(&v);
}
