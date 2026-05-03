mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::Value;

fn render_resolved_lens(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let l = &resp["result"];
    let sl = l["range"]["start"]["line"].as_u64().unwrap_or(0);
    let title = l["command"]["title"].as_str().unwrap_or("<unresolved>");
    let cmd = l["command"]["command"].as_str().unwrap_or("");
    format!("L{sl}: {title} [{cmd}]")
}

#[tokio::test]
async fn lens_for_method_ref_count() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class Service {
    public function run(): void {}
}
$s = new Service();
$s->run();
$s->run();
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:13: 1 reference [editor.action.showReferences]
        L2:20-L2:23: 2 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_phpunit_named_test_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class FooTest {
    public function testItWorks(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:13: 0 references [editor.action.showReferences]
        L2:20-L2:31: 0 references [editor.action.showReferences]
        L2:20-L2:31: ▶ Run test [php-lsp.runTest]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_test_attribute() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class FooTest {
    #[Test]
    public function it_works(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:13: 0 references [editor.action.showReferences]
        L3:20-L3:28: 0 references [editor.action.showReferences]
        L3:20-L3:28: ▶ Run test [php-lsp.runTest]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_fqn_test_attribute() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class FooTest {
    #[PHPUnit\Framework\Attributes\Test]
    public function it_works(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:13: 0 references [editor.action.showReferences]
        L3:20-L3:28: 0 references [editor.action.showReferences]
        L3:20-L3:28: ▶ Run test [php-lsp.runTest]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_at_test_docblock() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class FooTest {
    /** @test */
    public function it_works(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:13: 0 references [editor.action.showReferences]
        L3:20-L3:28: 0 references [editor.action.showReferences]
        L3:20-L3:28: ▶ Run test [php-lsp.runTest]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_interface_with_implementations() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
interface Animal {}
class Dog implements Animal {}
class Cat implements Animal {}
"#,
        )
        .await;
    expect![[r#"
        L1:10-L1:16: 0 references [editor.action.showReferences]
        L1:10-L1:16: 2 implementations [editor.action.showReferences]
        L2:6-L2:9: 0 references [editor.action.showReferences]
        L3:6-L3:9: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_abstract_class_with_subclass() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
abstract class Shape {}
class Circle extends Shape {}
"#,
        )
        .await;
    expect![[r#"
        L1:15-L1:20: 0 references [editor.action.showReferences]
        L1:15-L1:20: 1 implementation [editor.action.showReferences]
        L2:6-L2:12: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_trait_with_usages() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
trait Loggable {
    public function log(): void {}
}
class A { use Loggable; }
class B { use Loggable; }
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:14: 0 references [editor.action.showReferences]
        L1:6-L1:14: 2 implementations [editor.action.showReferences]
        L2:20-L2:23: 0 references [editor.action.showReferences]
        L4:6-L4:7: 0 references [editor.action.showReferences]
        L5:6-L5:7: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_overriding_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class Base {
    public function greet(): string { return 'hi'; }
}
class Child extends Base {
    public function greet(): string { return 'hello'; }
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:10: 0 references [editor.action.showReferences]
        L2:20-L2:25: 0 references [editor.action.showReferences]
        L4:6-L4:11: 0 references [editor.action.showReferences]
        L5:20-L5:25: 0 references [editor.action.showReferences]
        L5:20-L5:25: overrides Base::greet [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_enum_with_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
enum Suit {
    case Hearts;
    public function label(): string { return 'h'; }
}
"#,
        )
        .await;
    expect![[r#"
        L1:5-L1:9: 0 references [editor.action.showReferences]
        L2:9-L2:15: 0 references [editor.action.showReferences]
        L3:20-L3:25: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_counts_enum_case_usages() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
enum Suit {
    case Hearts;
    case Spades;
}
$a = Suit::Hearts;
$b = Suit::Hearts;
"#,
        )
        .await;
    expect![[r#"
        L1:5-L1:9: 2 references [editor.action.showReferences]
        L2:9-L2:15: 2 references [editor.action.showReferences]
        L3:9-L3:15: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_method_overriding_used_trait() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
trait Loggable {
    public function log(): void {}
}
class Service {
    use Loggable;
    public function log(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:14: 0 references [editor.action.showReferences]
        L1:6-L1:14: 1 implementation [editor.action.showReferences]
        L2:20-L2:23: 0 references [editor.action.showReferences]
        L4:6-L4:13: 0 references [editor.action.showReferences]
        L6:20-L6:23: 0 references [editor.action.showReferences]
        L6:20-L6:23: overrides Loggable::log [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_class_property() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class User {
    public string $name = '';
    public function rename(string $new): void { $this->name = $new; }
    public function who(): string { return $this->name; }
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:10: 0 references [editor.action.showReferences]
        L2:19-L2:23: 2 references [editor.action.showReferences]
        L3:20-L3:26: 0 references [editor.action.showReferences]
        L4:20-L4:23: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_for_promoted_constructor_property() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class Dog {
    public function __construct(public int $age) {}
    public function birthday(): void { $this->age++; }
    public function years(): int { return $this->age; }
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:9: 0 references [editor.action.showReferences]
        L2:20-L2:31: 0 references [editor.action.showReferences]
        L2:44-L2:47: 2 references [editor.action.showReferences]
        L3:20-L3:28: 0 references [editor.action.showReferences]
        L4:20-L4:25: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn lens_counts_references_across_files() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"//- /lib.php
<?php
function shared(): void {}
//- /a.php
<?php
shared();
//- /b.php
<?php
shared();
shared();
"#,
        )
        .await;
    expect!["L1:9-L1:15: 3 references [editor.action.showReferences]"].assert_eq(&out);
}

#[tokio::test]
async fn code_lens_resolve_roundtrips_run_test_lens() {
    let mut server = TestServer::new().await;
    server
        .open(
            "test.php",
            "<?php\nclass FooTest { public function testItWorks(): void {} }\n",
        )
        .await;

    let lenses = server.code_lens("test.php").await["result"]
        .as_array()
        .cloned()
        .expect("expected code lens array");
    let run_test_lens = lenses
        .iter()
        .find(|l| l["command"]["command"] == "php-lsp.runTest")
        .cloned()
        .expect("expected a php-lsp.runTest lens");

    let resp = server
        .client()
        .request("codeLens/resolve", run_test_lens)
        .await;
    expect!["L1: ▶ Run test [php-lsp.runTest]"].assert_eq(&render_resolved_lens(&resp));
}
