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

// ── declaration: open-file paths ────────────────────────────────────────────

/// Cursor on a method call resolves to the abstract declaration in a parent
/// class — exercises `find_abstract_declaration` for `StmtKind::Class` with
/// `m.is_abstract`, not for an interface.
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

/// Cursor on a usage of an interface name (in `implements`) resolves to the
/// interface's name range — exercises the `i.name == word` branch in
/// `find_abstract_declaration`.
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

/// Concrete free function — no abstract counterpart exists, so the first pass
/// yields nothing and we fall through to `find_any_declaration`'s
/// `StmtKind::Function` arm.
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

/// Plain (non-abstract) class — second pass resolves the class name.
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

/// Trait method — `find_any_declaration` matches `StmtKind::Trait` and walks
/// its members. No abstract counterpart, so first pass returns None.
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

/// Trait property — exercises the `Property` arm of the trait body, including
/// the `$`-prefix stripping (`bare = word.strip_prefix('$')`).
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

/// Trait constant — exercises the `ClassConst` arm of `StmtKind::Trait`.
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

/// Cursor on an enum name resolves to its declaration via the second pass
/// (`StmtKind::Enum(e) if e.name == word`).
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

/// Interface constant — exercises the `ClassConst` arm of `StmtKind::Interface`
/// in `find_any_declaration` (the abstract pass doesn't look at constants).
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

/// Cross-file: cursor in the implementation file, declaration lives in a
/// separate (also-opened) interface file. Both files are opened so this hits
/// the open-doc path, not the index fallback.
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

/// Interface inside a braced namespace — exercises the `NamespaceBody::Braced`
/// recursion in `find_abstract_declaration`.
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
//
// `goto_declaration` only looks at *open* docs. When the cursor's word is
// undefined in every open doc, the backend falls through to
// `goto_declaration_from_index`, which serves results from `FileIndex` entries
// built by the workspace scan. To exercise that path over the wire we write
// files to disk, start a rooted server, wait for the scan to finish, and only
// `did_open` the caller — so the declaration target is index-only.

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

/// No abstract counterpart: free function served via the index second-pass.
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

/// No abstract counterpart: plain class name served via the index second-pass.
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
    //                          ^^^^^^^^^^^^^^^^^^^^ def
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

/// Variable goto-definition must jump to the first assignment in scope, not the
/// use-site. $0 is on a read; the `def` annotation marks the initial assignment.
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

/// Cross-file goto-definition for a free function (not a class), exercises the
/// `StmtKind::Function` arm of `scan_statements` via `other_docs`.
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

    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected a definition location: {resp:?}"
    );
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    assert!(
        loc["uri"]
            .as_str()
            .unwrap()
            .ends_with("AbstractController.php"),
        "must jump to AbstractController::render(), got: {loc:?}"
    );
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
async fn implementation_anonymous_class_no_panic() {
    let mut s = TestServer::new().await;
    let out = s
        .check_implementation(
            r#"<?php
interface Greet$0er {}
$obj = new class implements Greeter {};
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

// Cursor on the parent class: `class Circle extends Base$0 implements Shape {}`
// must return exactly one location, same as searching for the interface.
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

// Without a `use` import the handler passes fqn=None.
// A class that writes `extends \Animal` (backslash-qualified) refers to the
// global-namespace Animal — it must be found when searching for a global-
// namespace interface `Animal` (no namespace declaration).
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

// With `use App\Animal` the handler resolves fqn="App\Animal".
// A class doing `extends \App\Animal` (fully-qualified with leading backslash)
// must be found.
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

// Same as above but the class writes `extends App\Animal` (no leading `\`).
// `name_matches` strips the leading `\` from the repr before comparing.
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

// When fqn is provided, the short-name form `extends Animal` must still match
// so that classes in the same namespace (which omit the namespace prefix) are
// included alongside FQN-using classes.
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

// ── implementation name-range regression ──────────────────────────────────────
//
// When the implementing class name also appears (as a word) in a string literal
// *before* the class declaration in the file, the old global `name_range` scan
// --- PHP 8 attribute alias expansion ---

/// `#[ORM\Column]` with `use Doctrine\ORM\Mapping as ORM` must jump to the
/// `Column` class, not silently fail.  The PSR-4 fallback must expand the alias
/// prefix before resolving the FQN.
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
    let result = &resp["result"];
    assert!(!result.is_null(), "expected definition: {resp:?}");
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    let uri = loc["uri"].as_str().unwrap_or("");
    assert!(
        uri.ends_with("Mapping/Column.php"),
        "expected Column.php, got: {uri}"
    );
}

// (using `.to_string()`) found the literal occurrence instead of the declaration.
// The span-constrained fix restricts the search to the class stmt's span.

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
