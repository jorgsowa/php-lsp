//! Comprehensive go-to-definition / declaration / typeDefinition coverage.

use super::*;

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

/// Cross-file goto-definition for a namespace-free class.
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
async fn implementation_on_interface() {
    let mut s = TestServer::new().await;
    s.check_implementation_annotated(
        r#"<?php
interface Writ$0able { public function write(): void; }
class A implements Writable { public function write(): void {} }
//    ^ impl
class B implements Writable { public function write(): void {} }
//    ^ impl
"#,
    )
    .await;
}

#[tokio::test]
async fn implementation_on_abstract_class() {
    let mut s = TestServer::new().await;
    s.check_implementation_annotated(
        r#"<?php
abstract class Base$0 { abstract public function handle(): void; }
class ConcreteA extends Base { public function handle(): void {} }
//    ^^^^^^^^^ impl
class ConcreteB extends Base { public function handle(): void {} }
//    ^^^^^^^^^ impl
"#,
    )
    .await;
}

#[tokio::test]
async fn implementation_multiple_classes() {
    let mut s = TestServer::new().await;
    s.check_implementation_annotated(
        r#"<?php
interface Work$0er { public function execute(): void; }
class JobA implements Worker { public function execute(): void {} }
//    ^^^^ impl
class JobB implements Worker { public function execute(): void {} }
//    ^^^^ impl
class JobC implements Worker { public function execute(): void {} }
//    ^^^^ impl
"#,
    )
    .await;
}

#[tokio::test]
async fn implementation_cross_file() {
    let mut s = TestServer::new().await;
    s.check_implementation_annotated(
        r#"//- /src/Contract.php
<?php
interface Operat$0or { public function run(): void; }

//- /src/impl/Add.php
<?php
class Add implements Operator { public function run(): void {} }
//    ^^^ impl

//- /src/impl/Sub.php
<?php
class Sub implements Operator { public function run(): void {} }
//    ^^^ impl
"#,
    )
    .await;
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

#[tokio::test]
async fn implementation_interface_extends_single() {
    let mut s = TestServer::new().await;
    s.check_implementation_annotated(
        r#"<?php
interface Animal$0 {}
interface Dog extends Animal {}
//        ^^^ impl
"#,
    )
    .await;
}

#[tokio::test]
async fn implementation_interface_extends_multiple() {
    let mut s = TestServer::new().await;
    s.check_implementation_annotated(
        r#"<?php
interface Animal$0 {}
interface Dog extends Animal {}
//        ^^^ impl
interface Cat extends Animal {}
//        ^^^ impl
"#,
    )
    .await;
}

/// An implementor that satisfies an interface method purely by *inheriting*
/// it from an ancestor which itself does not declare `implements` — the
/// ancestor never overrides the method locally. Confirmed live gap against a
/// real ~15K-file codebase: 11 of 15 real implementors of
/// `Indexable::serializeToElasticsearchDocument` were invisible because they
/// relied on an inherited, non-overriding ancestor method. mir commit
/// 7b5ce9e8 ("indexed_method_implementations walks the inheritance chain")
/// closes this for the mir 0.61.0 bump.
#[tokio::test]
async fn implementation_via_inherited_non_overriding_method() {
    let mut s = TestServer::new().await;
    s.check_implementation_annotated(
        r#"<?php
interface Indexable$0 {
    public function serialize(): array;
}

abstract class DocumentBlock {
    public function serialize(): array { return []; }
}

final class DocumentBlockText extends DocumentBlock implements Indexable {}
//          ^^^^^^^^^^^^^^^^^ impl

final class DocumentBlockImage extends DocumentBlock implements Indexable {}
//          ^^^^^^^^^^^^^^^^^^ impl
"#,
    )
    .await;
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

/// A method call through an `object`-typed property holding an anonymous
/// class that composes a trait resolves to a same-named method on the
/// *enclosing* class instead of the trait's — even though the anonymous
/// class has no relation whatsoever to the enclosing class. Order-dependent
/// (reproduces with the trait defined before or after the class) but not
/// file-count-dependent, and unaffected by waiting for indexReady, so this
/// isn't a warm-up race — it looks like a fallback that name-matches
/// against the enclosing class when the anonymous class's actual member
/// (through the untyped `object`) can't be resolved directly. Found via a
/// real PHPUnit `getMockedXTrait()`-returns-`object` pattern in app-server
/// (`EntityGetterTrait/GetCreatedAtTest.php` and
/// `OffsetPaginationFieldsTest.php`).
#[tokio::test]
#[ignore = "known bug: a method call through an object-typed property \
            holding an anonymous class + trait resolves to a same-named \
            method on the enclosing class instead of the trait's own \
            method — the two are otherwise unrelated"]
async fn definition_through_object_property_resolves_to_enclosing_class_not_trait() {
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
    expect!["src/Outer.php:13:20-13:28"].assert_eq(&out);
}

/// PHP arrow-function parameters strictly shadow any outer variable of the
/// same name for the entire arrow-fn body — but goto-definition on a
/// shadowing use resolves to the *outer*, shadowed variable instead of the
/// arrow-fn's own parameter. Internally inconsistent, not just wrong: hover
/// on the exact same token correctly infers the parameter's declared type
/// (`Item`), proving the type-inference and goto-definition paths disagree
/// about which declaration this token even is. Found via a real
/// `fn(Catalog $catalog) => ...` case in app-server
/// (`GetCatalogService.php`) shadowing an outer `$catalog` of a different
/// type.
#[tokio::test]
#[ignore = "known bug: goto-definition on an arrow-fn parameter that shadows \
            an outer same-named variable resolves to the outer variable \
            instead of the parameter — hover on the same token correctly \
            uses the parameter's type, so the two paths disagree"]
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
    expect!["main.php:13:8-13:16"].assert_eq(&out);
}

/// A method returning `self` is lexically bound to the *declaring* class at
/// compile time — unlike `static`, it must NOT resolve using the calling
/// instance's actual (subclass) type. `Base::returnsSelf(): self` returns
/// `Base`, which has no `subOnly()` — a real static analyzer (PHPStan/Psalm)
/// flags `$sub->returnsSelf()->subOnly()` as calling an undefined method.
/// php-lsp instead confidently resolves it to `Sub::subOnly()`, silently
/// treating `self` as if it were late-static-bound `static`. Found via
/// app-server's `Document`/`Holders` hierarchy, which declares both a
/// `self`-returning factory and separate `static`-returning fluent methods.
#[tokio::test]
#[ignore = "known bug: a method declared to return `self` is resolved as if \
            it returned `static` (late static binding) — the next call in \
            the chain uses the calling subclass's members instead of the \
            declaring class's, which can point at a method that doesn't \
            exist on the declared return type at all"]
async fn definition_on_self_return_type_uses_subclass_members() {
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
    expect!["main.php:12:20-12:27"].assert_eq(&out);
}

/// A bare call to a name pulled in via `use function` must invoke the
/// imported function — but when a method of the same name also exists on
/// the enclosing class, the bare call resolves to the local method instead,
/// even though a bare call (no `$this->`/`self::`) can never mean "call my
/// own method" in PHP. The control case (`$this->shout(...)`, not included
/// here) correctly resolves to the local method, and `references` on the
/// local method correctly excludes the bare call site — the bug is isolated
/// to hover/goto-definition's callee resolution. Found via app-server's
/// `SentryLogWriter.php`, which imports `use function
/// Sentry\captureException` and also declares a same-named private method
/// that itself calls the bare (intended-to-be-imported) function.
#[tokio::test]
#[ignore = "known bug: a bare call to a use-function-imported name resolves \
            to a same-named local method instead of the imported function, \
            even though a bare call can never mean \"call my own method\" \
            in PHP"]
async fn definition_on_bare_call_prefers_local_method_over_use_function_import() {
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
    expect!["src/app.php:13:21-13:26"].assert_eq(&out);
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
    //                          ^^^^^^^^^^^^^^^^^^^^ def
        echo $na$0me;
    }
}
"#,
    )
    .await;
}

/// Untyped promoted param with only a `@param` docblock resolves to the param,
/// not to an unrelated class with the same property name.
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

/// Variable goto-definition jumps to the first assignment in scope.
#[tokio::test]
async fn definition_variable_jumps_to_first_occurrence() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
function foo() {
    $x = 1;
//  ^^ def
    return $x$0;
}
"#,
    )
    .await;
}

/// Goto-definition on a typed parameter variable must land on `$param`, not on
/// the type annotation. Previously, `p.span` (which starts at the type) was
/// used, so clicking `$x` in `Baz $x` would jump to `B` in `Baz`.
#[tokio::test]
async fn definition_variable_typed_param_lands_on_sigil() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Baz {}
function foo(Baz $x): void {
//               ^^ def
    $y = $x$0;
}
"#,
    )
    .await;
}

/// Goto-definition inside a method body with a typed parameter must land on
/// `$param`, not the type annotation.
#[tokio::test]
async fn definition_variable_method_param_lands_on_sigil() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Greeter {
    public function greet(string $name): string {
        //                       ^^^^^ def
        return $name$0;
    }
}
"#,
    )
    .await;
}

/// Goto-definition with cursor ON the parameter declaration itself (not in the
/// body) must still resolve to that same parameter.
#[tokio::test]
async fn definition_variable_cursor_on_param_declaration() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
function transform(int $val$0): int {
    //                 ^^^^ def
    return $val * 2;
}
"#,
    )
    .await;
}

/// Goto-definition for an untyped parameter `function foo($x)` must land on
/// `$x`, not fail or jump to an unrelated position.
#[tokio::test]
async fn definition_variable_untyped_param() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
function noop($x): mixed {
    //        ^^ def
    return $x$0;
}
"#,
    )
    .await;
}

/// Goto-definition for a typed parameter with a default value must land on
/// `$param`, not on the type or the default.
#[tokio::test]
async fn definition_variable_param_with_default() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
function greet(string $name = 'World'): string {
    //                ^^^^^ def
    return 'Hello ' . $name$0;
}
"#,
    )
    .await;
}

/// Goto-definition for the second of several typed params must land on the
/// correct `$param`, not on the first one.
#[tokio::test]
async fn definition_variable_second_typed_param() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
function add(int $a, int $b): int {
    //                   ^^ def
    return $a + $b$0;
}
"#,
    )
    .await;
}

/// Goto-definition on an enum case reference must jump to the case declaration.
#[tokio::test]
async fn definition_enum_case_same_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
enum Suit {
    case Hearts;
    //   ^^^^^^ def
}
$x = Suit::Hearts$0;
"#,
    )
    .await;
}

/// Goto-definition on an enum method reference must jump to the method declaration.
#[tokio::test]
async fn definition_enum_method_same_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
enum Color {
    case Red;
    public function label(): string { return ''; }
    //              ^^^^^ def
}
Color::Red->labe$0l();
"#,
    )
    .await;
}

/// Goto-definition on a symbol inside a braced namespace must find it.
#[tokio::test]
async fn definition_symbol_inside_braced_namespace() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
namespace App {
    function boot() {}
    //       ^^^^ def
    boo$0t();
}
"#,
    )
    .await;
}

/// Cross-file goto-definition for a free function (not a class).
#[tokio::test]
async fn definition_cross_file_free_function() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"//- /helpers.php
<?php
function helperFn() {}
//       ^^^^^^^^ def

//- /main.php
<?php
helperFn$0();
"#,
    )
    .await;
}

/// When a symbol is defined in both the current file and another file, the
/// current file's definition must be returned (current-file-first search order).
#[tokio::test]
async fn definition_current_file_takes_priority_over_other_files() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"//- /main.php
<?php
class Foo {}
//    ^^^ def
$f = new Foo$0();

//- /other.php
<?php
class Foo {}
"#,
    )
    .await;
}

/// Goto-definition on a regular (non-promoted) property access must jump to
/// the property declaration, not just the class declaration.
#[tokio::test]
async fn definition_regular_property_same_file() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Person {
    public string $name = '';
    //             ^^^^ def
}
$p = new Person();
$p->na$0me;
"#,
    )
    .await;
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
    expect!["main.php:2:0-2:0"].assert_eq(&out);
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
    expect!["main.php:3:0-3:0"].assert_eq(&out);
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
    expect!["Model.php:2:0-2:0"].assert_eq(&out);
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

/// Class name appearing in a string literal before its declaration must not confuse
/// the implementation search; the result must point to the actual class declaration.
#[tokio::test]
async fn implementation_class_name_correct_when_name_appears_in_earlier_string_literal() {
    let mut s = TestServer::new().await;
    s.check_implementation_annotated(
        r#"<?php
interface Logg$0able {}
$msg = 'Loggable is implemented by Logger right here';
class Logger implements Loggable {}
//    ^^^^^^ impl
"#,
    )
    .await;
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

/// Cursor on a method call via a typed variable resolves to the parent class
/// that declares the method, not the child class that inherits it.
#[tokio::test]
async fn definition_method_inherited_from_parent_via_variable() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Base {
    public function render(): void {}
    //              ^^^^^^ def
}
class Child extends Base {}

$c = new Child();
$c->ren$0der();
"#,
    )
    .await;
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

/// `from` is classified as a keyword token by the underlying lexer crate
/// (it's part of `yield from`), but PHP only reserves it directly after
/// `yield` — everywhere else, most commonly a backed enum's static factory
/// (`Suit::from('H')`), it's an ordinary method name. `is_php_keyword` must
/// not treat it as always-reserved, or this call site would incorrectly
/// resolve to nothing.
#[tokio::test]
async fn goto_definition_on_static_method_named_from_resolves() {
    let mut s = TestServer::new().await;
    s.check_definition_annotated(
        r#"<?php
class Suit {
    public static function from(string $value): self { return new self(); }
    //                     ^^^^ def
}
Suit::fr$0om('H');
"#,
    )
    .await;
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
#[ignore = "known bug: the PHPDoc documentation-only-token gate doesn't \
            cover hyphenated psalm/phpstan pseudo-types (non-empty-string, \
            class-string<T>, ...) — word_at_position splits on the hyphen, \
            and a resulting bareword segment that collides with a real \
            declaration elsewhere in the workspace resolves to it"]
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
    expect!["src/Unrelated.php:1:9-1:12"].assert_eq(&out);
}
