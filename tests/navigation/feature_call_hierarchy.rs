//! Call hierarchy — all tests go through the LSP wire protocol.

use super::*;

use expect_test::expect;

// ── call hierarchy: prepare ────────────────────────────────────────────────────

#[tokio::test]
async fn prepare_function_returns_function_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
function gree$0t(): void {}
"#,
        )
        .await;
    expect!["greet (Function) @ main.php:1:9"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_class_method_returns_method_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
class Mailer {
    public function sen$0d(): void {}
}
"#,
        )
        .await;
    expect!["send (Method) [Mailer] @ main.php:2:20"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_trait_method_returns_method_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
trait Timestampable {
    public function touc$0h(): void {}
}
"#,
        )
        .await;
    expect!["touch (Method) [Timestampable] @ main.php:2:20"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_enum_method_returns_method_item() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
enum Suit {
    case Hearts;
    public function lab$0el(): string { return 'x'; }
}
"#,
        )
        .await;
    expect!["label (Method) [Suit] @ main.php:3:20"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_unknown_symbol_returns_empty() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
$va$0r = 42;
"#,
        )
        .await;
    expect!["<empty>"].assert_eq(&out);
}

// ── call hierarchy: incoming ───────────────────────────────────────────────────

#[tokio::test]
async fn incoming_calls_lists_callers() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function leaf$0(): void {}
function caller(): void { leaf(); }
"#,
        )
        .await;
    expect!["caller @ main.php:2:9 fromRanges=[2:26-2:30]"].assert_eq(&out);
}

#[tokio::test]
async fn incoming_calls_empty_when_never_called() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function unuse$0d(): void {}
"#,
        )
        .await;
    expect!["<no calls>"].assert_eq(&out);
}

#[tokio::test]
async fn incoming_calls_multiple_callers() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function tar$0get(): void {}
function a(): void { target(); }
function b(): void { target(); }
"#,
        )
        .await;
    expect![[r#"
        a @ main.php:2:9 fromRanges=[2:21-2:27]
        b @ main.php:3:9 fromRanges=[3:21-3:27]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn incoming_calls_cross_file() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"//- /Service.php
<?php function proces$0s(): void {}
//- /Controller.php
<?php function handle(): void { process(); }
"#,
        )
        .await;
    expect!["handle @ Controller.php:0:15 fromRanges=[0:32-0:39]"].assert_eq(&out);
}

#[tokio::test]
async fn incoming_calls_from_file_scope() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function boota$0ble(): void {}
bootable();
"#,
        )
        .await;
    expect!["<file scope> @ main.php:2:0 fromRanges=[2:0-2:8]"].assert_eq(&out);
}

// ── call hierarchy: outgoing ───────────────────────────────────────────────────

#[tokio::test]
async fn outgoing_calls_lists_callees() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function leaf(): void {}
function caller$0(): void { leaf(); }
"#,
        )
        .await;
    expect!["leaf @ main.php:1:9 fromRanges=[2:26-2:30]"].assert_eq(&out);
}

/// First-class callable syntax (PHP 8.1 `foo(...)`) must count as an
/// outgoing call, same as a regular `foo()` invocation.
#[tokio::test]
async fn outgoing_calls_includes_first_class_callable() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function leaf(): void {}
function caller$0(): void { $f = leaf(...); }
"#,
        )
        .await;
    expect!["leaf @ main.php:1:9 fromRanges=[2:31-2:35]"].assert_eq(&out);
}

/// First-class callable syntax on a method call (`$obj->method(...)`).
#[tokio::test]
async fn outgoing_calls_includes_first_class_callable_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
class Greeter {
    public function hello(): void {}
}
class Service {
    public function run$0(Greeter $g): void { $f = $g->hello(...); }
}
"#,
        )
        .await;
    expect!["hello @ main.php:2:20 fromRanges=[5:53-5:58]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_empty_for_leaf_function() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function noo$0p(): void { $x = 1; }
"#,
        )
        .await;
    expect!["<no calls>"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_cross_file_callee() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"//- /main.php
<?php function orchest$0rate(): void { helper(); }
//- /helpers.php
<?php function helper(): void {}
"#,
        )
        .await;
    expect!["helper @ helpers.php:0:15 fromRanges=[0:37-0:43]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_deduplicates_repeated_callee() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function helper(): void {}
function caller$0(): void { helper(); helper(); }
"#,
        )
        .await;
    expect!["helper @ main.php:1:9 fromRanges=[2:26-2:32, 2:36-2:42]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_from_class_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function validate(): bool { return true; }
class Order {
    public function subm$0it(): void { validate(); }
}
"#,
        )
        .await;
    expect!["validate @ main.php:1:9 fromRanges=[3:37-3:45]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_from_enum_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function fmt(): string { return ''; }
enum Suit {
    public function lab$0el(): string { return fmt(); }
}
"#,
        )
        .await;
    expect!["fmt @ main.php:1:9 fromRanges=[3:45-3:48]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_includes_for_init_and_update() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function start(): int { return 0; }
function step(): void {}
function mai$0n(): void { for ($i = start(); $i < 10; step()) {} }
"#,
        )
        .await;
    expect![[r#"
        start @ main.php:1:9 fromRanges=[3:34-3:39]
        step @ main.php:2:9 fromRanges=[3:52-3:56]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_includes_static_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
class Cache {
    public static function warm(): void {}
}
function bootstra$0p(): void { Cache::warm(); }
"#,
        )
        .await;
    expect!["warm @ main.php:2:27 fromRanges=[4:36-4:40]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_inside_do_while() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function tick(): bool { return true; }
function pol$0l(): void { do {} while (tick()); }
"#,
        )
        .await;
    expect!["tick @ main.php:1:9 fromRanges=[2:37-2:41]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_inside_switch() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function action(): void {}
function dispa$0tch(int $x): void {
    switch ($x) {
        case 1: action(); break;
    }
}
"#,
        )
        .await;
    expect!["action @ main.php:1:9 fromRanges=[4:16-4:22]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_includes_args_of_new_expr() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function defaults(): array { return []; }
class Config {}
function boo$0t(): void { $c = new Config(defaults()); }
"#,
        )
        .await;
    expect!["defaults @ main.php:1:9 fromRanges=[3:40-3:48]"].assert_eq(&out);
}

#[tokio::test]
async fn outgoing_calls_inside_cast() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function measure(): float { return 1.5; }
function conv$0ert(): int { return (int) measure(); }
"#,
        )
        .await;
    expect!["measure @ main.php:1:9 fromRanges=[2:39-2:46]"].assert_eq(&out);
}

// ── additional call hierarchy edge cases ────────────────────────────────────

/// Recursive function must appear in its own incoming calls.
#[tokio::test]
async fn incoming_calls_includes_recursive_function() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function facto$0rial(int $n): int { return $n <= 1 ? 1 : $n * factorial($n - 1); }
"#,
        )
        .await;
    expect!["factorial @ main.php:1:9 fromRanges=[1:60-1:69]"].assert_eq(&out);
}

/// Method calling itself recursively must appear in incoming calls.
#[tokio::test]
async fn incoming_calls_includes_recursive_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
class TreeNode {
    public function trave$0rse(): void { $this->traverse(); }
}
"#,
        )
        .await;
    expect!["traverse @ main.php:2:20 fromRanges=[2:46-2:54]"].assert_eq(&out);
}

/// Outgoing calls must include recursive call within the function.
#[tokio::test]
async fn outgoing_calls_includes_recursive_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function recurs$0e(int $n): void { if ($n > 0) recurse($n - 1); }
"#,
        )
        .await;
    expect!["recurse @ main.php:1:9 fromRanges=[1:45-1:52]"].assert_eq(&out);
}

/// Calling a trait method must resolve to the trait method implementation.
#[tokio::test]
async fn incoming_calls_from_trait_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
trait Logger {
    public function lo$0g(): void {}
}
class Service {
    use Logger;
    public function run(): void { $this->log(); }
}
"#,
        )
        .await;
    expect!["run @ main.php:6:20 fromRanges=[6:41-6:44]"].assert_eq(&out);
}

/// Calling a trait method should show the trait method in outgoing calls.
#[tokio::test]
async fn outgoing_calls_from_class_using_trait() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
trait Logger {
    public function log(): void {}
}
class Service {
    use Logger;
    public function ru$0n(): void { $this->log(); }
}
"#,
        )
        .await;
    expect!["log @ main.php:2:20 fromRanges=[6:41-6:44]"].assert_eq(&out);
}

/// `use Trait { method as alias; }` — a call through the alias name must
/// resolve outgoing calls to the trait's real method, since the alias itself
/// never appears as a literal method declaration anywhere in the AST.
#[tokio::test]
async fn outgoing_calls_through_trait_method_alias() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
trait Logger {
    public function log(): void {}
}
class Service {
    use Logger { log as debugLog; }
    public function ru$0n(): void { $this->debugLog(); }
}
"#,
        )
        .await;
    expect!["log @ main.php:2:20 fromRanges=[6:41-6:49]"].assert_eq(&out);
}

/// `prepareCallHierarchy` invoked directly on an aliased call site (cursor on
/// the alias name, not the trait's real method name) must still resolve.
#[tokio::test]
async fn prepare_call_hierarchy_on_trait_method_alias_call_site() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
trait Logger {
    public function log(): void {}
}
class Service {
    use Logger { log as debugLog; }
    public function run(): void { $this->debu$0gLog(); }
}
"#,
        )
        .await;
    expect!["log (Method) [Logger] @ main.php:2:20"].assert_eq(&out);
}

/// Method with no calls to other functions must report empty outgoing calls.
#[tokio::test]
async fn outgoing_calls_from_leaf_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
class Leaf {
    public function disu$0se(): void { $x = 1; $y = $x + 1; }
}
"#,
        )
        .await;
    expect!["<no calls>"].assert_eq(&out);
}

/// Methods can call parent methods, tracked as outgoing calls.
#[tokio::test]
async fn outgoing_calls_includes_method_from_parent() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
class Base {
    public function setup(): void {}
}
class Child extends Base {
    public function ini$0t(): void { $this->setup(); }
}
"#,
        )
        .await;
    expect!["setup @ main.php:2:20 fromRanges=[5:42-5:47]"].assert_eq(&out);
}

/// Static method calls must be tracked in outgoing calls.
#[tokio::test]
async fn outgoing_calls_includes_static_call_multiple_times() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
class Utils { public static function uuid(): string { return ''; } }
class Factory {
    public function crea$0te(): void { $a = Utils::uuid(); $b = Utils::uuid(); }
}
"#,
        )
        .await;
    expect!["uuid @ main.php:1:37 fromRanges=[3:49-3:53, 3:69-3:73]"].assert_eq(&out);
}

/// Multiple calls to the same function are deduplicated in outgoing calls.
#[tokio::test]
async fn outgoing_calls_deduplicates_method_calls() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
class Helper { public function process(): void {} }
class Worker {
    public function wo$0rk(): void { $h = new Helper(); $h->process(); $h->process(); }
}
"#,
        )
        .await;
    expect!["process @ main.php:1:31 fromRanges=[3:58-3:65, 3:73-3:80]"].assert_eq(&out);
}

/// Nullsafe method calls must be included in outgoing calls.
#[tokio::test]
async fn outgoing_calls_includes_nullsafe_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
class Service { public function handle(): void {} }
class Proxy {
    public function deleate$0(): void { $svc?->handle(); }
}
"#,
        )
        .await;
    expect!["handle @ main.php:1:32 fromRanges=[3:45-3:51]"].assert_eq(&out);
}

/// Calls in conditional expressions must be detected.
#[tokio::test]
async fn outgoing_calls_in_ternary_expression() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function check(): bool { return true; }
function decid$0e(): void { $x = check() ? 1 : 2; }
"#,
        )
        .await;
    expect!["check @ main.php:1:9 fromRanges=[2:31-2:36]"].assert_eq(&out);
}

/// Calls in array elements must be detected.
#[tokio::test]
async fn outgoing_calls_in_array_elements() {
    let mut s = TestServer::new().await;
    let out = s
        .check_outgoing_calls(
            r#"<?php
function item(): string { return ''; }
function col$0llect(): array { return [item(), item()]; }
"#,
        )
        .await;
    expect!["item @ main.php:1:9 fromRanges=[2:37-2:41, 2:45-2:49]"].assert_eq(&out);
}

/// Prepare on non-existent function must return empty.
#[tokio::test]
async fn prepare_nonexistent_function_returns_empty() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
fooba$0r();
"#,
        )
        .await;
    expect!["<empty>"].assert_eq(&out);
}

/// Incoming calls deduplicates multiple calls from same location (count not repeated).
#[tokio::test]
async fn incoming_calls_deduplicates_per_caller() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
function helpe$0r(): void {}
function caller(): void { helper(); helper(); helper(); }
"#,
        )
        .await;
    // All three calls are from the same caller, deduplicated to one incoming call
    expect!["caller @ main.php:2:9 fromRanges=[2:26-2:32, 2:36-2:42, 2:46-2:52]"].assert_eq(&out);
}

/// Methods in inherited classes must show up in incoming calls.
#[tokio::test]
async fn incoming_calls_to_inherited_method() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
class Base {
    public function execu$0te(): void {}
}
class Derived extends Base {
    public function run(): void { $this->execute(); }
}
"#,
        )
        .await;
    expect!["run @ main.php:5:20 fromRanges=[5:41-5:48]"].assert_eq(&out);
}

// ── selectionRange containment regression tests ────────────────────────────────
//
// When a method name text also appears in an earlier string literal inside the
// same class/trait/enum, the old global `name_range` scan returned the literal's
// position, violating the selectionRange ⊆ range invariant required by LSP.

#[tokio::test]
async fn prepare_class_method_selection_range_not_stolen_by_earlier_string_literal() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
class Mailer {
    private string $template = 'please send this message';
    public function sen$0d(): void {}
}
"#,
        )
        .await;
    expect!["send (Method) [Mailer] @ main.php:3:20"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_trait_method_selection_range_not_stolen_by_earlier_string_literal() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
trait Sendable {
    private string $default = 'send message to recipients';
    public function sen$0d(): void {}
}
"#,
        )
        .await;
    expect!["send (Method) [Sendable] @ main.php:3:20"].assert_eq(&out);
}

#[tokio::test]
async fn prepare_enum_method_selection_range_not_stolen_by_earlier_string_literal() {
    let mut s = TestServer::new().await;
    let out = s
        .check_prepare_call_hierarchy(
            r#"<?php
enum Status {
    case Active;
    const LABEL = 'process the item';
    public function proce$0ss(): string { return ''; }
}
"#,
        )
        .await;
    expect!["process (Method) [Status] @ main.php:4:20"].assert_eq(&out);
}

#[tokio::test]
async fn incoming_calls_enclosing_method_selection_range_not_stolen_by_earlier_string_literal() {
    let mut s = TestServer::new().await;
    let out = s
        .check_incoming_calls(
            r#"<?php
class EventBus {
    private string $routing = 'dispatch to all listeners';
    public function broadca$0st(): void {}
    public function dispatch(): void {
        $this->broadcast();
    }
}
"#,
        )
        .await;
    expect!["dispatch @ main.php:4:20 fromRanges=[5:15-5:24]"].assert_eq(&out);
}
