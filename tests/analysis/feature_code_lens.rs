use super::*;

use expect_test::expect;
use serde_json::{Value, json};

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
    // The two `implements Animal` clauses count as references to the
    // interface — same posting-list semantics as find-references.
    expect![[r#"
        L1:10-L1:16: 2 implementations [editor.action.showReferences]
        L1:10-L1:16: 2 references [editor.action.showReferences]
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
        L1:15-L1:20: 1 implementation [editor.action.showReferences]
        L1:15-L1:20: 1 reference [editor.action.showReferences]
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
        L1:6-L1:14: 2 implementations [editor.action.showReferences]
        L1:6-L1:14: 2 references [editor.action.showReferences]
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
        L1:6-L1:10: 1 reference [editor.action.showReferences]
        L2:20-L2:25: 0 references [editor.action.showReferences]
        L4:6-L4:11: 0 references [editor.action.showReferences]
        L5:20-L5:25: 0 references [editor.action.showReferences]
        L5:20-L5:25: overrides Base::greet [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

/// A method redeclared two levels below its original declaration (the direct
/// parent doesn't redeclare it) must still get an "overrides" lens pointing
/// at the ancestor that actually declares it — `parent_method_location`
/// previously only checked the direct supertype.
#[tokio::test]
async fn lens_for_overriding_method_two_levels_up() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
class Grandparent {
    public function greet(): string { return 'hi'; }
}
class Parent1 extends Grandparent {}
class Child extends Parent1 {
    public function greet(): string { return 'hello'; }
}
"#,
        )
        .await;
    expect![[r#"
        L1:6-L1:17: 1 reference [editor.action.showReferences]
        L2:20-L2:25: 0 references [editor.action.showReferences]
        L4:6-L4:13: 1 reference [editor.action.showReferences]
        L5:6-L5:11: 0 references [editor.action.showReferences]
        L6:20-L6:25: 0 references [editor.action.showReferences]
        L6:20-L6:25: overrides Grandparent::greet [editor.action.showReferences]"#]]
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
        L1:6-L1:14: 1 implementation [editor.action.showReferences]
        L1:6-L1:14: 1 reference [editor.action.showReferences]
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

#[tokio::test]
async fn code_lens_resolve_bare_lens_roundtrips() {
    let mut server = TestServer::new().await;
    server.open("bare.php", "<?php\nfunction test() {}\n").await;

    let bare_lens = json!({
        "range": {
            "start": { "line": 1, "character": 0 },
            "end":   { "line": 1, "character": 8 }
        }
    });

    let resp = server
        .client()
        .request("codeLens/resolve", bare_lens.clone())
        .await;

    assert!(resp["error"].is_null(), "error: {resp:?}");
    // Snapshot the rendered lens — establishes regression baseline for the resolve path.
    expect!["L1: <unresolved> []"].assert_eq(&render_resolved_lens(&resp));
    // Range must survive unchanged (render_resolved_lens only shows line, not full range).
    assert_eq!(
        resp["result"]["range"], bare_lens["range"],
        "range must roundtrip"
    );
}

#[tokio::test]
async fn code_lens_resolve_all_lenses_preserve_structure() {
    let mut server = TestServer::new().await;
    server
        .open(
            "test.php",
            "<?php\nclass TestClass { \n  public function test1(): void {} \n  public function test2(): void {} \n}\n",
        )
        .await;

    let lenses = server.code_lens("test.php").await["result"]
        .as_array()
        .cloned()
        .expect("expected code lens array");

    let mut rendered = Vec::new();
    for lens in &lenses {
        let resp = server
            .client()
            .request("codeLens/resolve", lens.clone())
            .await;
        assert!(resp["error"].is_null(), "resolve error for lens: {lens:?}");
        assert!(
            resp["result"]["range"].is_object(),
            "range must be preserved"
        );
        rendered.push(render_resolved_lens(&resp));
    }
    let out = rendered.join("\n");
    expect![[r#"
        L1: 0 references [editor.action.showReferences]
        L2: 0 references [editor.action.showReferences]
        L2: ▶ Run test [php-lsp.runTest]
        L3: 0 references [editor.action.showReferences]
        L3: ▶ Run test [php-lsp.runTest]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_lens_resolve_with_null_command() {
    let mut server = TestServer::new().await;
    server.open("null_cmd.php", "<?php").await;

    let lens = json!({
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 5 }
        },
        "command": null
    });

    let resp = server
        .client()
        .request("codeLens/resolve", lens.clone())
        .await;
    assert!(resp["error"].is_null());
    // Spec requires the server not to inject a command when `null` was sent.
    expect!["L0: <unresolved> []"].assert_eq(&render_resolved_lens(&resp));
}

#[tokio::test]
async fn code_lens_resolve_preserves_data_field() {
    let mut server = TestServer::new().await;
    server.open("data.php", "<?php").await;

    let lens = json!({
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 0, "character": 5 }
        },
        "command": {
            "title": "Test",
            "command": "test.run"
        },
        "data": { "testId": "123", "nested": { "value": "data" } }
    });

    let resp = server
        .client()
        .request("codeLens/resolve", lens.clone())
        .await;
    assert!(resp["error"].is_null());
    let result = &resp["result"];
    assert_eq!(
        result["data"], lens["data"],
        "data field must be preserved exactly"
    );
}

#[tokio::test]
async fn code_lens_resolve_is_idempotent() {
    let mut server = TestServer::new().await;
    server
        .open(
            "test.php",
            "<?php\nclass FooTest { public function testIt(): void {} }\n",
        )
        .await;

    let lenses = server.code_lens("test.php").await["result"]
        .as_array()
        .cloned()
        .expect("expected lenses");
    let first_lens = lenses[0].clone();

    let resolved_once = server
        .client()
        .request("codeLens/resolve", first_lens.clone())
        .await;
    let resolved_twice = server
        .client()
        .request("codeLens/resolve", resolved_once["result"].clone())
        .await;

    // Snapshot the result after first resolve — establishes regression baseline.
    expect!["L1: 0 references [editor.action.showReferences]"]
        .assert_eq(&render_resolved_lens(&resolved_once));
    assert_eq!(
        resolved_once["result"], resolved_twice["result"],
        "calling resolve twice must return identical results (idempotent)"
    );
}

/// Enum methods must get a code lens for references, just like class methods.
#[tokio::test]
async fn lens_for_enum_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_lens(
            r#"<?php
enum Suit {
    public function label(): string { return 'x'; }
}
"#,
        )
        .await;
    expect![[r#"
        L1:5-L1:9: 0 references [editor.action.showReferences]
        L2:20-L2:25: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&out);
}

/// Every lens that fires `editor.action.showReferences` must supply exactly
/// three arguments `[uri, position, locations]`. This guards against the bug
/// class where a lens has `arguments: None` and silently does nothing.
#[tokio::test]
async fn lens_show_references_always_has_three_arguments() {
    let mut s = TestServer::new().await;
    s.open(
        "test.php",
        r#"<?php
interface Animal { public function speak(): string; }
trait Barker { public function bark(): string { return 'woof'; } }
class Dog implements Animal {
    use Barker;
    public string $breed = '';
    public function speak(): string { return 'woof'; }
}
function topLevel(): void {}
"#,
    )
    .await;
    let resp = s.code_lens("test.php").await;
    let lenses = resp["result"].as_array().cloned().unwrap_or_default();
    let mut seen_any = false;
    for lens in &lenses {
        let Some(cmd) = lens["command"].as_object() else {
            continue;
        };
        if cmd["command"].as_str() == Some("editor.action.showReferences") {
            seen_any = true;
            let args = &cmd["arguments"];
            assert!(!args.is_null(), "lens {:?} missing arguments", cmd["title"]);
            assert_eq!(
                args.as_array().map(|a| a.len()),
                Some(3),
                "lens {:?} must pass [uri, position, locations]",
                cmd["title"]
            );
            assert!(
                args[2].is_array(),
                "3rd argument (locations) must be an array"
            );
        }
    }
    assert!(
        seen_any,
        "fixture must produce at least one showReferences lens"
    );
}

/// Verify code_lens returns correct lenses even when the document was edited
/// before the request. The write bumps `write_rev`; the handler captures the
/// new rev at request-start, so the sweep completes normally (no spurious
/// cancellation — the revision is stable for the entire spawn_blocking call).
#[tokio::test]
async fn code_lens_correct_after_preceding_write() {
    let mut s = TestServer::new().await;

    // Open, then change — write_rev is bumped by the change.
    s.open("main.php", "<?php\nclass Alpha {}\n").await;
    s.change(
        "main.php",
        2,
        "<?php\nclass Alpha {}\nfunction helperFn(): void {}\n",
    )
    .await;

    // code_lens after the write: revision is stable during spawn_blocking,
    // so lenses must be produced (not suppressed by stale-cancel logic).
    let resp = s.code_lens("main.php").await;
    expect![[r#"
        L1:6-L1:11: 0 references [editor.action.showReferences]
        L2:9-L2:17: 0 references [editor.action.showReferences]"#]]
    .assert_eq(&render_code_lens(&resp));
}
