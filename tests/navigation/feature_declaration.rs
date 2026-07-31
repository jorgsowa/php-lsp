//! `textDocument/declaration` — jump to abstract or interface declaration of a symbol.
//!
//! Comprehensive E2E tests covering:
//! - Interface method declarations (declaration ≠ definition)
//! - Abstract class and trait method declarations
//! - Concrete fallback cases (declaration == definition)
//! - Cross-file scenarios
//! - Edge cases (unknown symbol, empty file)

use super::*;

use expect_test::expect;

// ── Interface method declarations ──────────────────────────────────────────

#[tokio::test]
async fn interface_method_from_concrete_impl() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
interface Logger {
    public function log(string $msg): void;
    //              ^^^ decl
}
class FileLogger implements Logger {
    public function log$0(string $msg): void {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn interface_method_from_call_site() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
interface Logger {
    public function log(string $msg): void;
    //              ^^^ decl
}
class FileLogger implements Logger {
    public function log(string $msg): void {}
}
$logger = new FileLogger();
$logger->log$0('hello');
"#,
    )
    .await;
}

#[tokio::test]
async fn interface_method_on_declaration_site() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
interface Logger {
    public function log$0(string $msg): void;
    //              ^^^ decl
}
class FileLogger implements Logger {
    public function log(string $msg): void {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn cross_file_interface_method() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"//- /Logger.php
<?php
interface Logger {
    public function log(string $msg): void;
    //              ^^^ decl
}

//- /FileLogger.php
<?php
class FileLogger implements Logger {
    public function log$0(string $msg): void {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn two_interfaces_same_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
interface A {
    public function handle(): void;
}
interface B {
    public function handle(): void;
}
class Handler implements A, B {
    public function handle$0(): void {}
}
"#,
        )
        .await;
    expect![[r#"main.php:2:20-2:26"#]].assert_eq(&out);
}

#[tokio::test]
async fn interface_name_itself() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
interface Logger$0 {
//        ^^^^^^ decl
    public function log(): void;
}
"#,
    )
    .await;
}

// ── Abstract class method declarations ─────────────────────────────────────

#[tokio::test]
async fn abstract_method_from_concrete_subclass() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
abstract class Base {
    abstract public function build(): void;
    //                       ^^^^^ decl
}
class Impl extends Base {
    public function build$0(): void {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn abstract_method_on_declaration_site() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
abstract class Base {
    abstract public function build$0(): void;
    //                       ^^^^^ decl
}
class Impl extends Base {
    public function build(): void {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn cross_file_abstract_method() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"//- /Base.php
<?php
abstract class Base {
    abstract public function build(): void;
    //                       ^^^^^ decl
}

//- /Impl.php
<?php
class Impl extends Base {
    public function build$0(): void {}
}
"#,
    )
    .await;
}

// ── Abstract trait methods (bug case) ───────────────────────────────────────

#[tokio::test]
async fn abstract_trait_method_from_using_class() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
trait Renderable {
    abstract public function render(): string;
    //                       ^^^^^^ decl
}
class Page {
    use Renderable;
    public function render$0(): string { return ''; }
}
"#,
    )
    .await;
}

// ── Concrete fallback (declaration == definition) ──────────────────────────

#[tokio::test]
async fn concrete_function_falls_back_to_definition() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
function greet(): string { return 'hi'; }
//       ^^^^^ decl
greet$0();
"#,
    )
    .await;
}

#[tokio::test]
async fn concrete_class_name() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
class Widget {
//    ^^^^^^ decl
    public function show(): void {}
}
$w = new Widget$0();
"#,
    )
    .await;
}

#[tokio::test]
async fn trait_name_falls_back() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
trait Loggable$0 {
//    ^^^^^^^^ decl
    public function log(): void {}
}
class Page {
    use Loggable;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn enum_name_falls_back() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
enum Suit$0 {
//   ^^^^ decl
    case Hearts;
    public function label(): string { return 'H'; }
}
"#,
    )
    .await;
}

// ── Cross-file fallback ────────────────────────────────────────────────────

#[tokio::test]
async fn cross_file_function_fallback() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"//- /helpers.php
<?php
function validate(string $x): bool { return true; }
//       ^^^^^^^^ decl

//- /main.php
<?php
$ok = validate$0('test');
"#,
    )
    .await;
}

// ── Constants and enum members ────────────────────────────────────────────

#[tokio::test]
async fn class_constant_declaration() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
class Config {
    const DEBUG$0 = true;
    //    ^^^^^ decl
}
"#,
    )
    .await;
}

#[tokio::test]
async fn enum_case_declaration() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
enum Status {
    case Active$0;
    //   ^^^^^^ decl
    case Inactive;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn enum_constant_declaration() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
enum Suit {
    case Hearts;
    const MAX_VALUE$0 = 100;
    //    ^^^^^^^^^ decl
}
"#,
    )
    .await;
}

#[tokio::test]
async fn class_property_declaration() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
class User {
    public string $name$0;
    //             ^^^^ decl
    public function getName(): string { return $this->name; }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn trait_property_declaration() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
trait Timestampable {
    protected string $created$0;
    //                ^^^^^^^ decl
}
class Post {
    use Timestampable;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn property_cursor_on_usage_finds_declaration() {
    let mut s = TestServer::new().await;
    s.check_declaration_annotated(
        r#"<?php
class User {
    public string $email;
    //             ^^^^^ decl
}
$u = new User();
$u->email$0 = 'test@example.com';
"#,
    )
    .await;
}

// ── Edge cases ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_word_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
class Foo {}
$x = new Undefined$0Class();
"#,
        )
        .await;
    expect![[r#"<none>"#]].assert_eq(&out);
}

#[tokio::test]
async fn variable_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
$x$0 = 42;
"#,
        )
        .await;
    expect![[r#"<none>"#]].assert_eq(&out);
}

#[tokio::test]
async fn empty_file_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
$0"#,
        )
        .await;
    expect![[r#"<none>"#]].assert_eq(&out);
}

// ── Keyword tokens (regression) ───────────────────────────────────────────
//
// A bare PHP keyword can never be a declaration name. Before the
// `is_bare_keyword_at` gate, `decls_by_name.get(&word)` always missed for
// these, so every keyword click fell through to the exhaustive
// `any_declaration_in_file` scan over every workspace file.

#[tokio::test]
async fn keyword_token_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_declaration(
            r#"<?php
abstra$0ct class Foo {
    abstract public function build(): void;
}
"#,
        )
        .await;
    expect![[r#"<none>"#]].assert_eq(&out);
}

/// Same as `keyword_token_returns_none` but against unopened background
/// files, so the request must go through `goto_declaration_from_index`'s
/// exhaustive fallback scan rather than the open-doc AST pass.
#[tokio::test]
async fn keyword_in_unopened_workspace_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..10 {
        std::fs::write(
            tmp.path().join(format!("Noise{i}.php")),
            format!(
                "<?php\nabstract class Noise{i} {{\n    abstract public function run(): void;\n}}\n"
            ),
        )
        .unwrap();
    }
    let caller_src =
        "<?php\nabstract class Caller {\n    abstract public function run(): void;\n}\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "abstract class Caller", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["<none>"].assert_eq(&out);
}

// ── Stub-index fallback (unopened file) ──────────────────────────────────────
//
// Tests for declaration resolution through FileIndex entries (unopened files).
// These use tempdir to write files to disk, start a rooted server that scans
// them, then only open the caller file so the declaration target is index-only.

/// Abstract method in unopened parent class.
#[tokio::test]
async fn declaration_from_unopened_abstract_method() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Animal.php"),
        "<?php\nabstract class Animal {\n    abstract public function speak(): string;\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction call(Animal $a): string { return $a->speak(); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "speak()", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Animal.php:2:29-2:34"].assert_eq(&out);
}

/// Interface method in unopened interface.
#[tokio::test]
async fn declaration_from_unopened_interface_method() {
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

/// Interface name in unopened interface.
#[tokio::test]
async fn declaration_from_unopened_interface_name() {
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

/// Free function in unopened file.
#[tokio::test]
async fn declaration_from_unopened_function() {
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

/// Class name in unopened class.
#[tokio::test]
async fn declaration_from_unopened_class_name() {
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

/// Method in unopened class.
#[tokio::test]
async fn declaration_from_unopened_method() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Service.php"),
        "<?php\nclass Service {\n    public function execute(): void {}\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction run(Service $s): void { $s->execute(); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "execute()", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Service.php:2:20-2:27"].assert_eq(&out);
}

/// Property in unopened class (previously unimplemented for index path).
#[tokio::test]
async fn declaration_from_unopened_property() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Entity.php"),
        "<?php\nclass Entity {\n    public string $name = '';\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction get(Entity $e): string { return $e->name; }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "->name", 0);
    let ch = ch + "->".len() as u32; // move cursor to start of property name
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Entity.php:2:19-2:23"].assert_eq(&out);
}

/// Abstract method in unopened trait.
#[tokio::test]
async fn declaration_from_unopened_trait_abstract_method() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Loggable.php"),
        "<?php\ntrait Loggable {\n    abstract public function record(): void;\n}\n",
    )
    .unwrap();
    let caller_src = "<?php\nfunction output(): void { $x->record(); }\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("caller.php", caller_src).await;

    let (_, line, ch) = s.locate("caller.php", "record()", 0);
    let resp = s.declaration("caller.php", line, ch).await;
    let out = common::render_locations(&resp, &s.uri(""));
    expect!["Loggable.php:2:29-2:35"].assert_eq(&out);
}
