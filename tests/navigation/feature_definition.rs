//! Comprehensive go-to-definition / declaration / typeDefinition coverage.

use super::*;

use expect_test::expect;
use serde_json::json;

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

// ── declaration: open-file paths ────────────────────────────────────────────

/// Cursor on a method call resolves to the abstract declaration in the parent class.
#[tokio::test]
async fn declaration_on_abstract_class_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
abstract class Base { abstract public function build(): void; }
class Impl extends Base { public function build(): void {} }
$x = new Impl();
$x->bui$0ld();
"#,
        )
        .await;
    expect!["main.php:1:47-1:52"].assert_eq(&out);
}

/// Cursor on an interface name used in `implements` resolves to the interface declaration.
#[tokio::test]
async fn declaration_on_interface_name_usage() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
interface Writable { public function write(): void; }
class A implements Writ$0able {}
"#,
        )
        .await;
    expect!["main.php:1:10-1:18"].assert_eq(&out);
}

/// Declaration on a concrete free function (no abstract counterpart).
#[tokio::test]
async fn declaration_falls_back_to_concrete_function() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
function greet(): void {}
gre$0et();
"#,
        )
        .await;
    expect!["main.php:1:9-1:14"].assert_eq(&out);
}

/// Declaration on a plain (non-abstract) class resolves to the class name.
#[tokio::test]
async fn declaration_falls_back_to_class_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
class Widget {}
$w = new Wid$0get();
"#,
        )
        .await;
    expect!["main.php:1:6-1:12"].assert_eq(&out);
}

/// Declaration on a trait method resolves to the trait member.
#[tokio::test]
async fn declaration_on_trait_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
trait Greetable { public function hello(): string { return ''; } }
class A { use Greetable; }
$a = new A();
$a->hel$0lo();
"#,
        )
        .await;
    expect!["main.php:1:34-1:39"].assert_eq(&out);
}

/// Declaration on a trait property resolves to the property declaration.
#[tokio::test]
async fn declaration_on_trait_property() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
trait Named { public string $name = ''; }
class A { use Named; }
$a = new A();
$a->na$0me;
"#,
        )
        .await;
    expect!["main.php:1:29-1:33"].assert_eq(&out);
}

/// Declaration on a trait constant resolves to the constant declaration.
#[tokio::test]
async fn declaration_on_trait_constant() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
trait Versioned { const VERSION = '1.0'; }
class A { use Versioned; }
echo A::VERS$0ION;
"#,
        )
        .await;
    expect!["main.php:1:24-1:31"].assert_eq(&out);
}

/// Cursor on an enum name resolves to the enum declaration.
#[tokio::test]
async fn declaration_on_enum_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
enum Suit { case Hearts; }
$s = Su$0it::Hearts;
"#,
        )
        .await;
    expect!["main.php:1:5-1:9"].assert_eq(&out);
}

/// Declaration on an interface constant resolves to the constant declaration.
#[tokio::test]
async fn declaration_on_interface_constant() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
interface Limits { const MAX = 100; }
echo Limits::MA$0X;
"#,
        )
        .await;
    expect!["main.php:1:25-1:28"].assert_eq(&out);
}

/// Cross-file: declaration in a separately-opened abstract class file.
#[tokio::test]
async fn declaration_cross_file_abstract_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"//- /Animal.php
<?php
abstract class Animal {
    abstract public function speak(): string;
}

//- /Cat.php
<?php
class Cat extends Animal {
    public function speak(): string { return 'meow'; }
}
$c = new Cat();
$c->spe$0ak();
"#,
        )
        .await;
    expect!["Animal.php:2:29-2:34"].assert_eq(&out);
}

/// Word at cursor that doesn't match any declaration in any open doc returns
/// no location.
#[tokio::test]
async fn declaration_returns_none_for_unknown_word() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
nonexistent_$0func();
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// Declaration on a method inside a braced namespace resolves correctly.
#[tokio::test]
async fn declaration_inside_braced_namespace() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
namespace App {
    interface Logger { public function log(): void; }
    class FileLogger implements Logger { public function log(): void {} }
}
namespace App {
    $f = new FileLogger();
    $f->lo$0g();
}
"#,
        )
        .await;
    expect!["main.php:2:39-2:42"].assert_eq(&out);
}

// ── declaration: stub-index fallback (file on disk, not opened) ─────────────

/// Abstract method declaration served from a not-opened parent class.
#[tokio::test]
async fn declaration_from_index_finds_abstract_method() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Animal.php"),
        "<?php\nabstract class Animal {\n    abstract public function speak(): string;\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction call_speak(Animal $a): string { return $a->speak(); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "speak()", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Animal.php:2:29-2:34"].assert_eq(&out);
}

/// Interface method declaration served from a not-opened interface file.
#[tokio::test]
async fn declaration_from_index_finds_interface_method() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Logger.php"),
        "<?php\ninterface Logger {\n    public function log(string $msg): void;\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction emit(Logger $l, string $m): void { $l->log($m); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "log($m)", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Logger.php:2:20-2:23"].assert_eq(&out);
}

/// Interface name (as a type hint) served from a not-opened interface file.
#[tokio::test]
async fn declaration_from_index_finds_interface_name() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Logger.php"),
        "<?php\ninterface Logger {\n    public function log(): void;\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction emit(Logger $l): void { $l; }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "Logger $l", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Logger.php:1:10-1:16"].assert_eq(&out);
}

/// Free function declaration served from an indexed (not-opened) file.
#[tokio::test]
async fn declaration_from_index_falls_back_to_function() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("helpers.php"),
        "<?php\nfunction format_name(string $s): string { return $s; }\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction caller(): string { return format_name('x'); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "format_name('x')", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["helpers.php:1:9-1:20"].assert_eq(&out);
}

/// Plain class declaration served from an indexed (not-opened) file.
#[tokio::test]
async fn declaration_from_index_falls_back_to_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Widget.php"),
        "<?php\nclass Widget {\n    public function render(): void {}\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction make(): Widget { return new Widget(); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "new Widget", 0);
    let ch = ch + "new ".len() as u32;
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Widget.php:1:6-1:12"].assert_eq(&out);
}

/// Trait abstract method declaration served from unopened trait file.
#[tokio::test]
async fn declaration_from_index_finds_trait_abstract_method() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Renderable.php"),
        "<?php\ntrait Renderable {\n    abstract public function render(): string;\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction output(): string { $x->render(); return ''; }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "render()", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Renderable.php:2:29-2:35"].assert_eq(&out);
}

/// Enum case declaration served from unopened enum file.
#[tokio::test]
async fn declaration_from_index_finds_enum_case() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Status.php"),
        "<?php\nenum Status {\n    case Active;\n    case Inactive;\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction check(Status $s): bool { return $s === Status::Active; }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "Active", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Status.php:1:5-1:11"].assert_eq(&out);
}

/// Enum constant declaration served from unopened enum file.
#[tokio::test]
async fn declaration_from_index_finds_enum_constant() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Config.php"),
        "<?php\nenum Config {\n    const DEBUG = true;\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction debug(): bool { return Config::DEBUG; }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "DEBUG", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Config.php:1:5-1:11"].assert_eq(&out);
}

/// Class constant declaration served from unopened class file.
#[tokio::test]
async fn declaration_from_index_finds_class_constant() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("AppConfig.php"),
        "<?php\nclass AppConfig {\n    const VERSION = '1.0';\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction version(): string { return AppConfig::VERSION; }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "VERSION", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["AppConfig.php:1:6-1:15"].assert_eq(&out);
}

/// Word at cursor that doesn't match any open doc *or* any indexed entry
/// returns no location.
#[tokio::test]
async fn declaration_from_index_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    let caller_src = "<?php\nfunction caller(): void { totally_missing(); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "totally_missing", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["<none>"].assert_eq(&out);
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
async fn type_definition_on_class_method_param() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
class Config {}
class Service {
    public function setup(Config $c$0fg): void {}
}
"#,
        )
        .await;
    expect!["main.php:1:6-1:12"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_on_constructor_param() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
class Logger {}
class Service {
    public function __construct(Logger $l$0og) {}
}
"#,
        )
        .await;
    expect!["main.php:1:6-1:12"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_on_nullable_param() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
class Database {}
function connect(?Database $d$0b): void {}
"#,
        )
        .await;
    expect!["main.php:1:6-1:14"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_cross_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"//- /src/Model.php
<?php
class User {}

//- /src/service.php
<?php
function save(User $u$0ser): void {}
"#,
        )
        .await;
    expect!["src/Model.php:1:6-1:10"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_on_interface_typed_var() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
interface Writer { public function write(): void; }
class FileWriter implements Writer { public function write(): void {} }
$w = new FileWriter();
$$0w;
"#,
        )
        .await;
    expect!["main.php:2:6-2:16"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_scalar_returns_empty() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
function greet(string $n$0ame): void {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_concrete_class_returns_empty() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
class Concret$0e { public function action(): void {} }
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

// ── Keyword tokens (regression) ───────────────────────────────────────────
//
// A bare PHP keyword is never a resolvable symbol. `implementation` resolves
// a garbage FQN like `App\abstract` for one, which mir's `commit_defs_for_
// matching` then hunts for via a full-workspace text-mention scan; `goto_
// definition` can jump to the *wrong* place instead — mir's `ClassReference`
// span for a class starts at its first modifier token, so `abstract` in
// `abstract class Foo` used to resolve to `Foo` itself.

#[tokio::test]
async fn goto_definition_on_class_modifier_keyword_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
abstra$0ct class Foo {
    abstract public function build(): void;
}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_on_class_modifier_keyword_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Shape {}
fina$0l class Circle implements Shape {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// A semi-reserved word (`final`) is a valid method name, so a cross-file
/// method literally named `final` can sit in the workspace index under that
/// bare name. Before the early bare-keyword bail-out in
/// `handle_goto_definition`, the class-modifier `final` here would fall
/// through the `ClassReference`/method-target checks (which do have their
/// own gate) all the way to the ungated `wi.find_declaration` lookup near
/// the end of the function, and "resolve" to that unrelated method.
#[tokio::test]
async fn goto_definition_on_class_modifier_keyword_ignores_cross_file_name_collision() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"//- /src/Other.php
<?php
class Other {
    public function final(): void {}
}

//- /src/main.php
<?php
fina$0l class Locked {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// A method call through an `object`-typed property (receiver type unknown to
/// mir) must not jump to a same-named method on the enclosing class.
#[tokio::test]
async fn definition_through_object_property_skips_enclosing_class_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"//- /src/Trait.php
<?php declare(strict_types=1);

trait GetsValue
{
    public function getTotal(): int
    {
        return 42;
    }
}

//- /src/Outer.php
<?php declare(strict_types=1);

final class Outer
{
    private object $sut;

    protected function setUp(): void
    {
        $this->sut = new class () {
            use GetsValue;
        };
    }

    public function getTotal(): void
    {
        self::assertEquals(42, $this->sut->getTo$0tal());
    }

    public static function assertEquals(int $expected, int $actual): void
    {
    }
}
"#,
        )
        .await;
    expect!["src/Trait.php:4:20-4:28"].assert_eq(&out);
}

/// With no other candidate anywhere, an unresolved member call yields nothing
/// rather than the enclosing class's same-named method.
#[tokio::test]
async fn definition_through_untyped_property_ignores_enclosing_class_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php declare(strict_types=1);

final class Outer
{
    private object $sut;

    public function getTotal(): int
    {
        return $this->sut->getTo$0tal();
    }
}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// An arrow-fn parameter shadows a same-named outer variable for the whole body.
#[tokio::test]
async fn definition_on_arrow_fn_param_ignores_outer_shadow() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php declare(strict_types=1);

final class Item
{
    public function __construct(public int $accountId)
    {
    }
}

final class Service
{
    public function run(): callable
    {
        $catalog = 'not an Item at all';

        return fn(Item $catalog) => $cata$0log->accountId;
    }
}
"#,
        )
        .await;
    expect!["main.php:15:23-15:31"].assert_eq(&out);
}

/// Same-file variant: the imported-name-free bare call resolves to the
/// namespaced function declared in the same file, not the same-named method.
#[tokio::test]
async fn definition_on_bare_call_resolves_same_file_function_not_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php

namespace App;

function shout(string $msg): void
{
    echo $msg;
}

class Logger
{
    public function log(string $msg): void
    {
        shou$0t($msg);
    }

    private function shout(string $msg): void
    {
    }
}
"#,
        )
        .await;
    expect!["main.php:4:9-4:14"].assert_eq(&out);
}

/// A method returning `self` is bound to the *declaring* class, unlike
/// `static`: `Base::returnsSelf(): self` returns `Base`, which has no
/// `subOnly()`, so the chained call must not resolve to `Sub::subOnly()`.
#[tokio::test]
async fn definition_on_self_return_type_ignores_subclass_members() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php

class Base
{
    public function returnsSelf(): self
    {
        return $this;
    }
}

class Sub extends Base
{
    public function subOnly(): string
    {
        return "sub";
    }
}

$sub = new Sub();
$a = $sub->returnsSelf()->subO$0nly();
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// A bare call to a `use function`-imported name resolves to the imported
/// function even when the enclosing class declares a same-named method.
#[tokio::test]
async fn definition_on_bare_call_resolves_use_function_import_not_local_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"//- /src/vendorlib.php
<?php

namespace Vendor;

function shout(string $msg): void
{
    echo "vendor: $msg";
}

//- /src/app.php
<?php

namespace App;

use function Vendor\shout;

class Logger
{
    public function log(string $msg): void
    {
        shou$0t($msg);
    }

    private function shout(string $msg): void
    {
        echo "local: $msg";
    }
}
"#,
        )
        .await;
    expect!["src/vendorlib.php:4:9-4:14"].assert_eq(&out);
}

#[tokio::test]
async fn definition_on_unknown_symbol_returns_null() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
$x = new Unkno$0wnClass();
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
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
    expect!["src/Model/User.php:4:6-4:10"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Goto-definition on a method call across files: `$user->greeting()` in
/// Greeter must jump to `User::greeting` in Model/User.php (line 12, char 20).
#[tokio::test]
async fn goto_definition_method_call_across_files() {
    let mut server = psr4_bring_up().await;
    psr4_open(&mut server, "src/Service/Greeter.php").await;
    let (_, line, ch) = server.locate("src/Service/Greeter.php", "greeting()", 0);

    let resp = server.definition("src/Service/Greeter.php", line, ch).await;
    expect!["src/Model/User.php:12:20-12:28"].assert_eq(&render_locations(&resp, &server.uri("")));
}

/// Receiver-aware dispatch: `$this->render()` must jump to the correct parent's
/// `render()` even when another unrelated class also defines `render()`.
#[tokio::test]
async fn definition_this_method_picks_correct_parent_not_unrelated_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("AbstractController.php"),
        "<?php\nclass AbstractController {\n    public function render(): string { return ''; }\n}\n",
    )
    .unwrap();
    // Unrelated class that also has render() — must NOT be returned.
    std::fs::write(
        tmp.path().join("BlockQuoteRenderer.php"),
        "<?php\nclass BlockQuoteRenderer {\n    public function render(): string { return ''; }\n}\n",
    )
    .unwrap();
    let ctrl_src = "<?php\nclass BlogController extends AbstractController {\n    public function index(): void { $this->render(); }\n}\n";
    std::fs::write(tmp.path().join("BlogController.php"), ctrl_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("BlogController.php", ctrl_src).await;

    let (_, line, ch) = s.locate("BlogController.php", "$this->render", 0);
    let ch = ch + "$this->".len() as u32;
    let resp = s.definition("BlogController.php", line, ch).await;

    expect!["AbstractController.php:2:20-2:26"].assert_eq(&render_locations(&resp, &s.uri("")));
}

#[tokio::test]
async fn implementation_enum_implements_interface() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface HasLabel$0 {}
enum Status: string implements HasLabel {
    case Active = 'active';
}
"#,
        )
        .await;
    expect!["main.php:2:5-2:11"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_class_implementing_multiple_interfaces() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Readable$0 {}
interface Writable {}
class Stream implements Readable, Writable {}
"#,
        )
        .await;
    expect!["main.php:3:6-3:12"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_class_that_extends_and_implements_interface_side() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Shape$0 {}
class Base {}
class Circle extends Base implements Shape {}
"#,
        )
        .await;
    expect!["main.php:3:6-3:12"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_partial_name_not_matched() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Countab$0le {}
class MyCountableList {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_no_implementors_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Orphan$0 {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_braced_namespace_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_implementation(
            r#"<?php
interface Runner$0 {}
namespace App {
    class Task implements Runner {}
}
"#,
        )
        .await;
    expect!["main.php:3:10-3:14"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_unbraced_namespace_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_implementation(
            r#"<?php
interface Worker$0 {}
namespace Jobs;
class BackgroundJob implements Worker {}
"#,
        )
        .await;
    expect!["main.php:3:6-3:19"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_anonymous_class_navigable() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Greet$0er {}
$obj = new class implements Greeter {};
"#,
        )
        .await;
    expect!["main.php:2:11-2:16"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_class_that_extends_and_implements_parent_side() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
class Base$0 {}
interface Shape {}
class Circle extends Base implements Shape {}
"#,
        )
        .await;
    expect!["main.php:3:6-3:12"].assert_eq(&out);
}

/// `extends \Animal` (backslash-qualified) must match a global-namespace `Animal`.
#[tokio::test]
async fn implementation_global_namespace_backslash_prefix_matched() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"//- /Animal.php
<?php
interface Animal$0 {}

//- /Dog.php
<?php
class Dog extends \Animal {}
"#,
        )
        .await;
    expect!["Dog.php:1:6-1:9"].assert_eq(&out);
}

/// `extends \App\Animal` (fully-qualified) matches when `use App\Animal` is in scope.
#[tokio::test]
async fn implementation_fqn_fully_qualified_extends_found() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"//- /search.php
<?php
use App\Animal;
function foo(Animal$0 $a): void {}

//- /Dog.php
<?php
class Dog extends \App\Animal {}
"#,
        )
        .await;
    expect!["Dog.php:1:6-1:9"].assert_eq(&out);
}

/// `extends App\Animal` (no leading `\`) matches when `use App\Animal` is in scope.
#[tokio::test]
async fn implementation_fqn_qualified_without_leading_backslash_found() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"//- /search.php
<?php
use App\Animal;
function foo(Animal$0 $a): void {}

//- /Dog.php
<?php
class Dog extends App\Animal {}
"#,
        )
        .await;
    expect!["Dog.php:1:6-1:9"].assert_eq(&out);
}

/// Short-name `extends Animal` matches even when a fully-qualified `use` is in scope.
#[tokio::test]
async fn implementation_fqn_short_name_still_matched() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"//- /search.php
<?php
use App\Animal;
function foo(Animal$0 $a): void {}

//- /Dog.php
<?php
class Dog extends Animal {}
"#,
        )
        .await;
    expect!["Dog.php:1:6-1:9"].assert_eq(&out);
}

/// When an exact fully-qualified subtype exists, it wins over legacy short-name
/// matches that would resolve to a different FQN in their own file.
#[tokio::test]
async fn implementation_exact_extends_suppresses_short_name_fallback() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"//- /search.php
<?php
use App\Animal;
function foo(Animal$0 $a): void {}

//- /Cat.php
<?php
class Cat extends \App\Animal {}

//- /Dog.php
<?php
namespace Other;
class Dog extends Animal {}
"#,
        )
        .await;
    expect!["Cat.php:1:6-1:9"].assert_eq(&out);
}

/// Same guard as the `extends` case, but through an `implements` clause. The
/// namespaced short-name relation is only a compatibility fallback when no
/// exact FQN relation is available.
#[tokio::test]
async fn implementation_exact_implements_suppresses_short_name_fallback() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_implementation(
            r#"<?php
interface Runner$0 {}
class Exact implements \Runner {}
namespace App {
    class Task implements Runner {}
}
"#,
        )
        .await;
    expect!["main.php:2:6-2:11"].assert_eq(&out);
}

// ── @method docblock go-to-definition ─────────────────────────────────────────

/// Calling a method declared only via `@method` on a typed parameter navigates
/// to the `@method` tag line in the same-file class docblock.
#[tokio::test]
async fn definition_doc_method_same_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
/**
 * @method User find(int $id)
 * @method static Builder where(string $col, mixed $val)
 */
class Model {}

function getUser(Model $m): void {
    $m->fin$0d(1);
}
"#,
        )
        .await;
    expect!["main.php:2:16-2:20"].assert_eq(&out);
}

/// `@method static` on a class: each tag navigates to its own line.
#[tokio::test]
async fn definition_doc_method_static_same_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
/**
 * @method User find(int $id)
 * @method static Builder where(string $col, mixed $val)
 */
class Model {}

function query(Model $m): void {
    $m->whe$0re("id", 1);
}
"#,
        )
        .await;
    expect!["main.php:3:26-3:31"].assert_eq(&out);
}

/// Cross-file: `@method` declared in an un-opened background-indexed file still
/// resolves definition when the caller is open.
#[tokio::test]
async fn definition_doc_method_cross_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Model.php"),
        "<?php\n/**\n * @method User find(int $id)\n */\nclass Model {}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction use_model(Model $m): void { $m->find(1); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "find(1)", 0);
    let resp = s.definition("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Model.php:2:16-2:20"].assert_eq(&out);
}

// ── @mixin docblock go-to-definition ─────────────────────────────────────────

/// A method defined in a mixin class resolves through the `@mixin` chain when
/// calling it on the class that declares `@mixin`.
#[tokio::test]
async fn definition_mixin_method_cross_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Macroable.php"),
        "<?php\nclass Macroable {\n    public function macro(string $name): void {}\n}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("Builder.php"),
        "<?php\n/**\n * @mixin Macroable\n */\nclass Builder {}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction use_builder(Builder $b): void { $b->macro('tap'); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "macro(", 0);
    let resp = s.definition("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Macroable.php:2:20-2:25"].assert_eq(&out);
}

// ── PHP 8 attribute alias expansion ──────────────────────────────────────────

/// `#[ORM\Column]` with `use Doctrine\ORM\Mapping as ORM` must jump to `Column`.
#[tokio::test]
async fn definition_php8_attribute_aliased_namespace() {
    use std::fs;
    let tmp = tempfile::tempdir().expect("TempDir");
    let root = tmp.path();

    fs::write(
        root.join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":""}}}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("Mapping")).unwrap();
    fs::write(
        root.join("Mapping/Column.php"),
        "<?php\nnamespace App\\Mapping;\nclass Column {}\n",
    )
    .unwrap();
    fs::write(
        root.join("Entity.php"),
        "<?php\nuse App\\Mapping as ORM;\n#[ORM\\Column]\nclass Entity {}\n",
    )
    .unwrap();

    let mut server = TestServer::with_root(root).await;
    server.wait_for_index_ready().await;
    let (entity_src, _, _) = server.locate("Entity.php", "<?php", 0);
    server.open("Entity.php", &entity_src).await;

    // cursor on `Column` inside `#[ORM\Column]` — line 2, char 5 (the 'C')
    let resp = server.definition("Entity.php", 2, 5).await;
    expect!["Mapping/Column.php:2:6-2:12"].assert_eq(&render_locations(&resp, &server.uri("")));
}

// ── Trait conflict resolution (insteadof) ────────────────────────────────────

/// `insteadof` conflict: go-to-definition navigates to the winning trait method.
#[tokio::test]
async fn insteadof_conflict_resolution_navigates_to_winning_trait() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
trait A {
    public function hello(): string { return 'A'; }
}
trait B {
    public function hello(): string { return 'B'; }
}
class MyClass {
    use A, B {
        B::hello insteadof A;  // B wins; A::hello is excluded
    }
}

$c = new MyClass();
$c->hel$0lo();
"#,
        )
        .await;
    // B wins the insteadof conflict; definition must point to B::hello on line 5.
    expect!["main.php:5:20-5:25"].assert_eq(&out);
}

// ── Facade / service-container gaps ──────────────────────────────────────────

/// A real method on a Facade-pattern class resolves normally.
#[tokio::test]
async fn facade_real_method_resolves() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
class Auth {
    public static function user(): mixed { return null; }
}

$u = Auth::us$0er();
"#,
        )
        .await;
    expect!["main.php:2:27-2:31"].assert_eq(&out);
}

/// A `__callStatic`-forwarded method (the Facade pattern) cannot be resolved
/// because the target is determined at runtime via getFacadeAccessor and the
/// service container. Definition returns nothing.
#[tokio::test]
async fn facade_callstatic_forwarded_method_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
class AuthFacade {
    protected static function getFacadeAccessor(): string { return 'auth'; }
    public static function __callStatic(string $method, array $args): mixed { return null; }
}

$u = AuthFacade::log$0in('admin@example.com', 'secret');
"#,
        )
        .await;
    // `login` is not defined on AuthFacade — the static call resolves nothing.
    expect!["<none>"].assert_eq(&out);
}

/// `app(Foo::class)` navigates to the class directly because the cursor lands
/// on `Foo`, not on the `app()` function itself.  Runtime container binding
/// is not resolved — the best the LSP can do is jump to the class declaration.
#[tokio::test]
async fn service_container_app_navigates_to_class() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
class UserRepository {
    public function find(int $id): mixed { return null; }
}

function bootstrap(): void {
    $repo = app(UserRep$0ository::class);
}
"#,
        )
        .await;
    // Cursor is on the class name token; definition resolves to the class declaration.
    expect!["main.php:1:6-1:20"].assert_eq(&out);
}

/// Both named and anonymous class implementors are returned by find-implementations.
#[tokio::test]
async fn implementation_anonymous_class_navigable_alongside_named() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Renderable$0 {
    public function render(): string;
}

$view = new class implements Renderable {
    public function render(): string { return '<div/>'; }
};

// Named implementor.
class HtmlView implements Renderable {
    public function render(): string { return '<p/>'; }
}
"#,
        )
        .await;
    // Both the named and anonymous implementors are returned.
    expect![[r#"
        main.php:10:6-10:14
        main.php:5:12-5:17"#]]
    .assert_eq(&out);
}

// ── inheritance, vendor autoload, trait aliases ───────────────────────────────

/// Definition on the interface in an `implements` clause (index-only).
#[tokio::test]
async fn definition_on_implements_target_from_index() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Shape.php"),
        "<?php\nnamespace App;\ninterface Shape {}\n",
    )
    .unwrap();
    let caller = "<?php\nnamespace App;\nclass Circle implements Shape {}\n";
    std::fs::write(tmp.path().join("Circle.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("Circle.php", caller).await;

    let (_, line, ch) = s.locate("Circle.php", "Shape {}", 0);
    let resp = s.definition("Circle.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Shape.php:2:10-2:15"].assert_eq(&out);
}

/// Definition on the parent class in an `extends` clause (index-only).
#[tokio::test]
async fn definition_on_extends_target_from_index() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Base.php"),
        "<?php\nnamespace App;\nclass Base {}\n",
    )
    .unwrap();
    let caller = "<?php\nnamespace App;\nclass Derived extends Base {}\n";
    std::fs::write(tmp.path().join("Derived.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("Derived.php", caller).await;

    let (_, line, ch) = s.locate("Derived.php", "Base {}", 0);
    let resp = s.definition("Derived.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Base.php:2:6-2:10"].assert_eq(&out);
}

/// Hover on the interface in an `implements` clause (index-only).
#[tokio::test]
async fn hover_on_implements_target_from_index() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Shape.php"),
        "<?php\nnamespace App;\ninterface Shape {}\n",
    )
    .unwrap();
    let caller = "<?php\nnamespace App;\nclass Circle implements Shape {}\n";
    std::fs::write(tmp.path().join("Circle.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("Circle.php", caller).await;

    let (_, line, ch) = s.locate("Circle.php", "Shape {}", 0);
    let resp = s.hover("Circle.php", line, ch).await;
    let out = render_hover(&resp);
    expect![[r#"
        ```php
        interface Shape
        ```"#]]
    .assert_eq(&out);
}

/// PSR-4 vendor class resolves from the index.
#[tokio::test]
async fn definition_psr4_vendor_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("vendor/acme/lib/src")).unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"Acme\\":"vendor/acme/lib/src/"}}}"#,
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("vendor/acme/lib/src/Client.php"),
        "<?php\nnamespace Acme;\nclass Client {}\n",
    )
    .unwrap();
    let caller = "<?php\nuse Acme\\Client;\n$c = new Client();\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "Client();", 0);
    let resp = s.definition("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["vendor/acme/lib/src/Client.php:2:6-2:12"].assert_eq(&out);
}

/// PSR-0 vendor class resolves from the index.
#[tokio::test]
async fn definition_psr0_vendor_class() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("vendor/acme/lib/src/Acme")).unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-0":{"Acme_":"vendor/acme/lib/src/"}}}"#,
    )
    .unwrap();
    // PSR-0: class `Acme_Client` maps to vendor/acme/lib/src/Acme/Client.php
    std::fs::write(
        tmp.path().join("vendor/acme/lib/src/Acme/Client.php"),
        "<?php\nclass Acme_Client {}\n",
    )
    .unwrap();
    let caller = "<?php\n$c = new Acme_Client();\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "Acme_Client();", 0);
    let resp = s.definition("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["vendor/acme/lib/src/Acme/Client.php:1:6-1:17"].assert_eq(&out);
}

/// Definition on a plain (un-aliased) trait method resolves correctly.
#[tokio::test]
async fn definition_on_trait_method_unaliased() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
trait BaseInit {
    public function init(int $x): void {}
}
class Query {
    use BaseInit;
    public function run(): void {
        $this->in$0it(1);
    }
}
"#,
        )
        .await;
    expect!["main.php:2:20-2:24"].assert_eq(&out);
}

/// Definition on a trait-aliased method call resolves to the original trait method.
#[tokio::test]
async fn definition_on_trait_aliased_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_definition(
            r#"<?php
trait BaseInit {
    public function __construct(int $x) {}
}
class Query {
    use BaseInit { __construct as __constructBase; }
    public function __construct() {
        $this->__construct$0Base(1);
    }
}
"#,
        )
        .await;
    expect!["main.php:2:20-2:31"].assert_eq(&out);
}

/// A circular inheritance graph (A extends B extends A) must not cause
/// `textDocument/definition` to hang. The request must complete and return
/// no result rather than looping indefinitely.
#[tokio::test]
async fn definition_circular_inheritance_does_not_hang() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_definition(
            r#"<?php
class A extends B {
    public function go(): void {}
}
class B extends A {}

$a = new A();
$a->mis$0sing();
"#,
        )
        .await;
    // Circular inheritance — no location can be returned.
    expect!["<none>"].assert_eq(&out);
}

// ── mir-backed implementation: aliased extends / FQN-qualified extends ───────

/// `class Child extends BaseAlias {}` where `use App\Base as BaseAlias;` must be
/// found when the cursor sits on `Base` with `use App\Base;` in scope.
/// The raw-name `subtypes_of` map stores "BaseAlias" but we search for "Base"/
/// "App\Base" — mir's resolved graph bridges the alias.
#[tokio::test]
async fn implementation_aliased_extends_cross_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"//- /App/Base.php
<?php
namespace App;
interface Base {}

//- /Other/Child.php
<?php
namespace Other;
use App\Base as BaseAlias;
class Child extends BaseAlias {}

//- /caller.php
<?php
use App\Base;
function test(Base$0 $x): void {}
"#,
        )
        .await;
    expect!["Other/Child.php:3:6-3:11"].assert_eq(&out);
}

/// `class Child extends \App\Base {}` (FQN-qualified with leading backslash) is found
/// when the cursor sits on `Base` with `use App\Base;` in scope. The AST-based
/// open-docs pass handles this via `name_matches`; the workspace-index fallback would
/// miss it because `subtypes_of` stores `"\App\Base"` but searches `"App\Base"`.
#[tokio::test]
async fn implementation_fqn_qualified_extends_cross_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"//- /App/Base.php
<?php
namespace App;
interface Base {}

//- /Child.php
<?php
class Child extends \App\Base {}

//- /caller.php
<?php
use App\Base;
function test(Base$0 $x): void {}
"#,
        )
        .await;
    expect!["Child.php:1:6-1:11"].assert_eq(&out);
}

/// `$x->bar()` after `if ($x instanceof Foo)` — resolved via mir's
/// `MethodCall` reference kind (`resolved_method_target` in
/// `backend/handlers/navigation.rs`), which already reflects flow-sensitive
/// narrowing. Pinned here as a currently-untested protocol path.
#[tokio::test]
async fn definition_receiver_narrowed_by_instanceof() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_definition(
            r#"<?php
class Base {}
class Foo extends Base {
    public function bar(): void {}
}
function test(Base $x): void {
    if ($x instanceof Foo) {
        $x->b$0ar();
    }
}
"#,
        )
        .await;
    expect!["main.php:3:20-3:23"].assert_eq(&out);
}

/// `$w->getName()` inside `foreach ($items as $w)` where `$items` is typed via
/// `@var list<Widget> $items`. Resolved the same way as the instanceof case
/// above — via mir's `MethodCall` reference kind, not `TypeMap`.
#[tokio::test]
async fn definition_receiver_foreach_element_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_definition(
            r#"<?php
class Widget {
    public function getName(): string { return ''; }
}
/** @var list<Widget> $items */
$items = get();
foreach ($items as $w) {
    $w->get$0Name();
}
"#,
        )
        .await;
    expect!["main.php:2:20-2:27"].assert_eq(&out);
}

/// `$x->getName()` where `$x`'s type comes from a bare `@var Alias $x` and
/// `Alias` is a `@psalm-type` declared on a *free function's own* docblock
/// (not a class).
#[tokio::test]
async fn definition_receiver_free_function_psalm_type_alias() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_definition(
            r#"<?php
class Widget {
    public function getName(): string { return ''; }
}
/**
 * @psalm-type Alias = Widget
 */
function test() {
    /** @var Alias $x */
    $x = get();
    $x->get$0Name();
}
"#,
        )
        .await;
    expect!["main.php:2:20-2:27"].assert_eq(&out);
}

/// Same as `definition_receiver_free_function_psalm_type_alias`, but with two
/// candidate classes sharing the same method name — only resolves correctly
/// if the alias is actually expanded, not just found by name.
#[tokio::test]
async fn definition_receiver_free_function_psalm_type_alias_ambiguous() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_definition(
            r#"<?php
class Gadget {
    public function getName(): string { return 'gadget'; }
}
class Widget {
    public function getName(): string { return 'widget'; }
}
/**
 * @psalm-type Alias = Widget
 */
function test() {
    /** @var Alias $x */
    $x = get();
    $x->get$0Name();
}
"#,
        )
        .await;
    expect!["main.php:5:20-5:27"].assert_eq(&out);
}

/// `__halt_compiler` is a hard reserved keyword (present on php.net's
/// reserved.keywords.php, previously missing from this project's list
/// entirely) — a bare cursor on it must never resolve to anything, same as
/// any other reserved word.
#[tokio::test]
async fn goto_definition_on_halt_compiler_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"<?php
__halt_$0compiler();
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// PHPDoc documentation-only tokens must never resolve via goto-definition
/// either — the same gate `references`/`hover` use (`is_unresolvable_docblock_token_at`),
/// exercised here against a cross-file name collision: an unrelated
/// top-level function literally named `T` sits elsewhere in the workspace.
#[tokio::test]
async fn goto_definition_on_template_parameter_name_ignores_cross_file_name_collision() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"//- /src/Unrelated.php
<?php
function T(): void {}

//- /src/Box.php
<?php
/**
 * @template T$0 of object
 */
class Box {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// Same gate, for the `$varName` half of a `@param` doc-tag body: an
/// unrelated property literally named `count` sits elsewhere in the
/// workspace.
#[tokio::test]
async fn goto_definition_on_param_doc_variable_name_ignores_cross_file_name_collision() {
    let mut s = TestServer::new().await;
    let out = s
        .check_definition(
            r#"//- /src/Unrelated.php
<?php
class Unrelated {
    public int $count = 0;
}

//- /src/Counter.php
<?php
class Counter {
    /**
     * @param int $count$0 starting value
     */
    public function __construct(int $count) {}
}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// A hyphenated psalm/phpstan pseudo-type inside a docblock (`non-empty-string`,
/// `class-string<T>`, etc.) isn't a valid PHP identifier — the hyphen forces
/// `word_at_position`'s purely textual scanner to split it into separate
/// bareword segments (`non`, `empty`, `string`). The existing PHPDoc-token
/// gate (`is_unresolvable_docblock_token_at`, ROADMAP item 0a) doesn't cover
/// this: it wasn't written with hyphenated compound pseudo-types in mind, so
/// a segment that happens to collide with a real, unrelated declaration
/// resolves to it. `class`/`interface`/`string`/`empty` can't be used to
/// prove this via collision — PHP reserves all four as identifiers, so no
/// real declaration can ever be named exactly that — but `non` is not
/// reserved, so an unrelated real `function non()` elsewhere in the
/// workspace makes an unambiguous collision.
#[tokio::test]
async fn references_on_hyphenated_pseudo_type_segment_finds_cross_file_name_collision() {
    let mut s = TestServer::new().await;
    let out = s
        .check_references(
            r#"//- /src/Unrelated.php
<?php
function non(): void {}

//- /src/Target.php
<?php
/**
 * @param non$0-empty-string $s
 */
function describe(string $s): void {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn implementation_after_emoji_uses_utf16_columns() {
    let mut s = TestServer::new().await;
    s.open(
        "main.php",
        "<?php\ninterface Shape {}\n/* 😀 */ class Square implements Shape {}\n",
    )
    .await;
    let resp = s.implementation("main.php", 1, 12).await;
    expect!["main.php:2:15-2:21"].assert_eq(&render_locations(&resp, &s.uri("")));
}
