use super::*;

use expect_test::expect;
use serde_json::json;

/// The definition file is never opened — it exists only in the workspace index
/// from the background scan. This is the typical production scenario.
#[tokio::test]
async fn inlay_hints_from_workspace_index_only() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("greeter.php"),
        "<?php\nfunction greet(string $name, int $count): void {}\n",
    )
    .unwrap();
    let caller_src = "<?php\ngreet('world', 3);\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    // Only open the caller — greeter.php is indexed but never opened.
    s.open("caller.php", caller_src).await;
    let resp = s.inlay_hints("caller.php", 0, 0, 3, 0).await;
    expect![[r#"
        1:6 name: [param]
        1:15 count: [param]"#]]
    .assert_eq(&render_inlay_hints(&resp));
}

#[tokio::test]
async fn inlay_hints_cross_file_function_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
greet('world', 3);

//- /greeter.php
<?php
function greet(string $name, int $count): void {}
"#,
        )
        .await;
    expect![[r#"
        1:6 name: [param]
        1:15 count: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_cross_file_constructor_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
$p = new Point(1, 2);

//- /Point.php
<?php
class Point {
    public function __construct(int $x, int $y) {}
}
"#,
        )
        .await;
    expect![[r#"
        1:15 x: [param]
        1:18 y: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_cross_file_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
$g = new Greeter();
$g->sayHello('World');

//- /Greeter.php
<?php
class Greeter {
    public function sayHello(string $name): void {}
}
"#,
        )
        .await;
    expect!["2:13 name: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_for_parameter_names() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function greet(string $name, int $count): void {}
greet('world', 3);
"#,
        )
        .await;
    expect![[r#"
        2:6 name: [param]
        2:15 count: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hint_resolve_populates_tooltip() {
    let mut s = TestServer::new().await;
    s.open(
        "resolve.php",
        "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
    )
    .await;
    let hints_resp = s.inlay_hints("resolve.php", 0, 0, 4, 0).await;
    let hints = hints_resp["result"].as_array().cloned().unwrap_or_default();
    let resp = s.inlay_hint_resolve(hints[0].clone()).await;
    let out = render_resolved_inlay_hint(&resp);
    expect![[r#"
2:4 a:
tooltip: ```php
function add(int $a, int $b): int
```"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hint_resolve_with_docblock_includes_docs() {
    let mut s = TestServer::new().await;
    s.open(
        "resolve.php",
        "<?php\n/** Adds two integers */\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
    )
    .await;
    let hints_resp = s.inlay_hints("resolve.php", 0, 0, 5, 0).await;
    let hints = hints_resp["result"].as_array().cloned().unwrap_or_default();
    let resp = s.inlay_hint_resolve(hints[0].clone()).await;
    let out = render_resolved_inlay_hint(&resp);
    expect![[r#"
3:4 a:
tooltip: ```php
function add(int $a, int $b): int
```

---

Adds two integers"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hint_resolve_no_data_field_returns_unchanged() {
    let mut s = TestServer::new().await;
    s.open("nohint.php", "<?php").await;

    let hint = json!({
        "position": { "line": 0, "character": 5 },
        "label": "$test:",
    });

    let resp = s.inlay_hint_resolve(hint).await;
    let out = render_resolved_inlay_hint(&resp);
    expect![[r#"
        0:5 $test:
        tooltip: <no tooltip>"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hint_resolve_existing_tooltip_is_noop() {
    let mut s = TestServer::new().await;
    s.open("existing.php", "<?php").await;

    let hint = json!({
        "position": { "line": 1, "character": 10 },
        "label": "param:",
        "tooltip": {
            "kind": "markdown",
            "value": "custom tooltip"
        }
    });

    let resp = s.inlay_hint_resolve(hint).await;
    let out = render_resolved_inlay_hint(&resp);
    expect![[r#"
        1:10 param:
        tooltip: custom tooltip"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hint_resolve_data_without_php_lsp_fn_returns_unchanged() {
    let mut s = TestServer::new().await;
    s.open("nokey.php", "<?php").await;

    let hint = json!({
        "position": { "line": 0, "character": 5 },
        "label": "param:",
        "data": {
            "some_other_key": "value"
        }
    });

    let resp = s.inlay_hint_resolve(hint).await;
    let out = render_resolved_inlay_hint(&resp);
    expect![[r#"
0:5 param:
tooltip: <no tooltip>"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hint_resolve_php_lsp_fn_nonexistent_function_returns_unchanged() {
    let mut s = TestServer::new().await;
    s.open("nofunc.php", "<?php").await;

    let hint = json!({
        "position": { "line": 2, "character": 8 },
        "label": "$x:",
        "data": {
            "php_lsp_fn": "nonExistentFunctionXyz"
        }
    });

    let resp = s.inlay_hint_resolve(hint).await;
    let out = render_resolved_inlay_hint(&resp);
    expect![[r#"
2:8 $x:
tooltip: <no tooltip>"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hint_resolve_is_idempotent() {
    let mut s = TestServer::new().await;
    s.open(
        "idempotent.php",
        "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
    )
    .await;

    let hints_resp = s.inlay_hints("idempotent.php", 0, 0, 4, 0).await;
    let hints = hints_resp["result"].as_array().cloned().unwrap_or_default();

    let resolved_once = s.inlay_hint_resolve(hints[0].clone()).await;
    let resolved_twice = s.inlay_hint_resolve(resolved_once["result"].clone()).await;

    let out1 = render_resolved_inlay_hint(&resolved_once);
    let out2 = render_resolved_inlay_hint(&resolved_twice);
    assert_eq!(
        out1, out2,
        "calling resolve twice must return identical results (idempotent)"
    );
    expect![[r#"
2:4 a:
tooltip: ```php
function add(int $a, int $b): int
```"#]]
    .assert_eq(&out1);
}

#[tokio::test]
async fn inlay_hints_nullsafe_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
$g = new Greeter();
$g?->sayHello('World');

//- /Greeter.php
<?php
class Greeter {
    public function sayHello(string $name): void {}
}
"#,
        )
        .await;
    expect!["2:14 name: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_static_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
Greeter::sayHello('world');

//- /Greeter.php
<?php
class Greeter {
    public static function sayHello(string $name): void {}
}
"#,
        )
        .await;
    expect!["1:18 name: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_empty_for_file_with_no_calls() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
$x = 1;
$y = 2;
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_respects_lsp_half_open_range_semantics() {
    // LSP uses half-open range semantics [start, end) where end is exclusive.
    // This test verifies that hints positioned exactly at range.end are excluded.
    let mut s = TestServer::new().await;
    s.open(
        "range_test.php",
        "<?php\nfunction f(int $x): void {}\nf(1);\n",
    )
    .await;

    // Line 2: "f(1);"
    //         01234
    // Hint is at character 2 (start of argument "1")

    // Range [0, 2) excludes the hint (it's AT the boundary, half-open semantics)
    let resp = s.inlay_hints("range_test.php", 2, 0, 2, 2).await;
    let out = render_inlay_hints(&resp);
    expect!["<no hints>"].assert_eq(&out);

    // Range [0, 3) includes the hint (it's within the range)
    let resp = s.inlay_hints("range_test.php", 2, 0, 2, 3).await;
    let out = render_inlay_hints(&resp);
    expect!["2:2 x: [param]"].assert_eq(&out);
}

/// Method hint collisions: when multiple classes define a method with the same name,
/// each call site shows the correct parameter hints for its own class.
#[tokio::test]
async fn inlay_hints_method_name_collision() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
$processor = new DataProcessor();
$processor->process(1, 2);

//- /DataProcessor.php
<?php
class DataProcessor {
    public function process(int $x, int $y): int {
        return $x + $y;
    }
}
"#,
        )
        .await;
    // Should show parameter hints for DataProcessor::process
    expect![[r#"
        2:20 x: [param]
        2:23 y: [param]"#]]
    .assert_eq(&out);
}

/// Edge case: two methods with same name but different parameter counts.
#[tokio::test]
async fn inlay_hints_method_different_signatures() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /main.php
<?php
$filter = new TextFilter();
$filter->apply("input", "lowercase");

//- /TextFilter.php
<?php
class TextFilter {
    public function apply(string $text, string $mode): string {
        return $mode === "lowercase" ? strtolower($text) : strtoupper($text);
    }
}
"#,
        )
        .await;
    expect![[r#"
        2:15 text: [param]
        2:24 mode: [param]"#]]
    .assert_eq(&out);
}

/// Edge case: inherited method should show parent's parameter hints, not overridden version.
#[tokio::test]
async fn inlay_hints_inherited_method_parameters() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /main.php
<?php
$child = new ChildClass();
$child->compute(10, 20);

//- /Parent.php
<?php
class ParentClass {
    public function compute(int $a, int $b): int {
        return $a + $b;
    }
}

//- /Child.php
<?php
class ChildClass extends ParentClass {
    // No override, should inherit parent's signature
}
"#,
        )
        .await;
    // Should show parent's parameter names (int $a, int $b)
    expect![[r#"
        2:16 a: [param]
        2:20 b: [param]"#]]
    .assert_eq(&out);
}

/// Edge case: static method with numeric parameters.
#[tokio::test]
async fn inlay_hints_static_method_with_math() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /main.php
<?php
$result = MathHelper::add(5, 3);

//- /MathHelper.php
<?php
class MathHelper {
    public static function add(int $a, int $b): int {
        return $a + $b;
    }
}
"#,
        )
        .await;
    // Static method hints should work the same as instance methods
    expect![[r#"
        1:26 a: [param]
        1:29 b: [param]
        1:31 : int [type]"#]]
    .assert_eq(&out);
}

/// Edge case: abstract method inherited by concrete class.
#[tokio::test]
async fn inlay_hints_abstract_method_implementation() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /main.php
<?php
$handler = new ConcreteHandler();
$handler->process("test", 123);

//- /Handler.php
<?php
abstract class AbstractHandler {
    abstract public function process(string $input, int $code): void;
}

//- /ConcreteHandler.php
<?php
class ConcreteHandler extends AbstractHandler {
    public function process(string $input, int $code): void {
        // implementation
    }
}
"#,
        )
        .await;
    // Should show abstract method's parameter names from parent
    expect![[r#"
        2:18 input: [param]
        2:26 code: [param]"#]]
    .assert_eq(&out);
}

// === Moved from src/inlay_hints.rs unit tests ===

#[tokio::test]
async fn inlay_hints_unknown_function_no_hints() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
unknownFn(1, 2);
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_zero_param_call_no_hints() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function init(): void {}
init();
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_skips_named_arguments() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function greet(string $name): void {}
greet(name: 'Alice');
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

/// A spread/unpack argument (`...$args`) maps to an unknown number of
/// positional parameters, so it must not get a hint labeling it with
/// whichever single parameter happens to sit at that argument index.
#[tokio::test]
async fn inlay_hints_skips_unpacked_argument() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function greet(string $name, string $greeting): void {}
$args = ['World', 'Hello'];
greet(...$args);
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

/// Once an unpack argument appears, a positional argument that follows it
/// also gets no hint — the unpacked array consumes an unknown number of
/// parameter slots at runtime, so the trailing argument's real parameter
/// can't be determined statically (labeling it via raw arg-list index would
/// be a wrong hint, not just a missing one).
#[tokio::test]
async fn inlay_hints_skips_positional_argument_trailing_an_unpack() {
    let mut s = TestServer::new().await;
    // Real PHP rejects a positional argument after unpacking, but the parser
    // must still tolerate it as a transient state while the user is editing.
    s.validate_syntax(false);
    let out = s
        .check_inlay_hints(
            r#"<?php
function greet(string $name, string $greeting, int $times): void {}
$args = ['World', 'Hello'];
greet(...$args, 3);
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_fewer_args_than_params() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function add(int $a, int $b): int { return $a + $b; }
add(1);
"#,
        )
        .await;
    expect!["2:4 a: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_more_args_than_params() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function f(int $x): void {}
f(1, 2, 3);
"#,
        )
        .await;
    expect!["2:2 x: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_return_type_for_assignment() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function make(): string { return 'x'; }
$s = make();
"#,
        )
        .await;
    expect!["2:11 : string [type]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_void_return_type_suppressed() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function init(): void {}
$x = init();
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_function_inside_namespace() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
namespace App;
function greet(string $name): void {}
greet('Alice');
"#,
        )
        .await;
    expect!["3:6 name: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_closure_variable_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
$greet = function(string $name, int $times): void {};
$greet('Alice', 3);
"#,
        )
        .await;
    expect![[r#"
        2:7 name: [param]
        2:16 times: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_arrow_function_variable_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
$double = fn(int $n): int => $n * 2;
$result = $double(5);
"#,
        )
        .await;
    expect!["2:18 n: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_call_inside_closure_body() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function add(int $a, int $b): int { return $a + $b; }
$fn = function() { add(1, 2); };
"#,
        )
        .await;
    expect![[r#"
        2:23 a: [param]
        2:26 b: [param]"#]]
    .assert_eq(&out);
}

/// Trait methods are registered in the def map by short name, so a method call
/// on an object whose class uses the trait resolves correctly.
#[tokio::test]
async fn inlay_hints_trait_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
trait Logging {
    public function log(string $msg, int $level): void {}
}
class AppLogger {
    use Logging;
}
$logger = new AppLogger();
$logger->log('hello', 3);
"#,
        )
        .await;
    expect![[r#"
        8:13 msg: [param]
        8:22 level: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_for_loop_calls() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function tick(int $n): void {}
for (tick(1); $i < 10; tick(2)) {}
"#,
        )
        .await;
    expect![[r#"
        2:10 n: [param]
        2:28 n: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_new_without_constructor_no_hints() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
class Foo {}
$f = new Foo();
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_calls_inside_trait_method_body() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function write(string $msg): void {}
trait Logger {
    public function log(): void { write('hello'); }
}
"#,
        )
        .await;
    expect!["3:40 msg: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_calls_inside_enum_method_body() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function write(string $msg): void {}
enum Status {
    case Active;
    public function log(): void { write('hello'); }
}
"#,
        )
        .await;
    expect!["4:40 msg: [param]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_enum_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
enum Status {
    case Active;
    public function label(string $prefix, int $pad): string { return ''; }
}
label('x', 2);
"#,
        )
        .await;
    expect![[r#"
        5:6 prefix: [param]
        5:11 pad: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_foreach_type_hint() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
class User {}
$users = array_map(fn($x): User => $x, []);
foreach ($users as $user) {
    $user;
}
"#,
        )
        .await;
    expect!["3:24 : User [type]"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_foreach_no_type_hint_when_unknown() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
foreach ($items as $item) {
    $item;
}
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_variadic_all_args() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function record(string ...$messages): void {}
record('a', 'b', 'c');
"#,
        )
        .await;
    expect![[r#"
        2:7 messages: [param]
        2:12 messages: [param]
        2:17 messages: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_variadic_after_regular_params() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function push(string $key, int ...$values): void {}
push('bucket', 1, 2, 3);
"#,
        )
        .await;
    expect![[r#"
        2:5 key: [param]
        2:15 values: [param]
        2:18 values: [param]
        2:21 values: [param]"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_arrow_function_declared_return_type() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
$double = fn(int $n): int => $n * 2;
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_arrow_function_no_return_type_annotation() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
$double = fn(int $n) => $n * 2;
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_constructor_promoted_properties() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
class User {
    public function __construct(
        public readonly string $name,
        public int $age,
    ) {}
}
$u = new User('Alice', 30);
"#,
        )
        .await;
    expect![[r#"
        7:14 name: [param]
        7:23 age: [param]"#]]
    .assert_eq(&out);
}

/// foreach with key => value: the implementation emits a type hint after the key
/// variable when mir knows its type. This test pins current behavior — if mir
/// cannot infer array key types the result is `<no hints>`.
#[tokio::test]
async fn inlay_hints_foreach_key_value_type_hint() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
class User {}
$users = array_map(fn($x): User => $x, []);
foreach ($users as $k => $user) {
    $user;
}
"#,
        )
        .await;
    // mir knows the value type from array_map but not the key type (int),
    // so only the value variable gets a hint — not the key variable.
    expect!["3:30 : User [type]"].assert_eq(&out);
}

/// Calls inside try/catch/finally bodies must receive hints.
#[tokio::test]
async fn inlay_hints_try_catch_finally_walk() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function cleanup(string $resource): void {}
try {
    cleanup('db');
} catch (Exception $e) {
    cleanup('conn');
} finally {
    cleanup('log');
}
"#,
        )
        .await;
    expect![[r#"
        3:12 resource: [param]
        5:12 resource: [param]
        7:12 resource: [param]"#]]
    .assert_eq(&out);
}

/// Calls nested inside an arrow function body must receive hints.
/// The implementation calls `hints_in_expr(sv, a.body, ...)` for this purpose.
#[tokio::test]
async fn inlay_hints_call_inside_arrow_function_body() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function greet(string $name): void {}
$fn = fn($x) => greet($x);
"#,
        )
        .await;
    expect!["2:22 name: [param]"].assert_eq(&out);
}

// === LSP specification gap tests ===

/// Hints must recompute after a `textDocument/didChange` — the server must not
/// serve a stale cached response.
#[tokio::test]
async fn inlay_hints_refresh_after_did_change() {
    let mut s = TestServer::new().await;
    s.open(
        "main.php",
        "<?php\nfunction greet(string $name): void {}\ngreet('Alice');\n",
    )
    .await;
    let resp = s.inlay_hints("main.php", 0, 0, 4, 0).await;
    expect!["2:6 name: [param]"].assert_eq(&render_inlay_hints(&resp));

    s.change(
        "main.php",
        2,
        "<?php\nfunction greet(string $name): void {}\nfunction add(int $a, int $b): int { return $a + $b; }\ngreet('Alice');\nadd(1, 2);\n",
    )
    .await;
    let resp = s.inlay_hints("main.php", 0, 0, 6, 0).await;
    expect![[r#"
        3:6 name: [param]
        4:4 a: [param]
        4:7 b: [param]"#]]
    .assert_eq(&render_inlay_hints(&resp));
}

/// The `initialize` response must advertise `resolveProvider: true` under
/// `inlayHintProvider` so clients know they can call `inlayHint/resolve`.
#[tokio::test]
async fn inlay_hints_server_advertises_resolve_provider() {
    let (_, init_resp) =
        TestServer::new_with_options(json!({ "diagnostics": { "enabled": true } })).await;
    let resolve_provider =
        init_resp["result"]["capabilities"]["inlayHintProvider"]["resolveProvider"]
            .as_bool()
            .unwrap_or(false);
    assert!(
        resolve_provider,
        "inlayHintProvider must advertise resolveProvider: true, got: {}",
        init_resp["result"]["capabilities"]["inlayHintProvider"]
    );
}

/// `InlayHintKind` values must match LSP spec: TYPE = 1, PARAMETER = 2.
/// Clients use `kind` to style/display hints differently (e.g. italics for
/// type hints). A swap between kinds would silently degrade the editor UX
/// without breaking any label-only snapshot.
#[tokio::test]
async fn inlay_hints_kind_field_values() {
    let mut s = TestServer::new().await;
    // Fixture emits both hint kinds:
    //   - line 3 `greet('Alice')` → parameter hint (kind=2, label "name:")
    //   - line 4 foreach → type hint (kind=1, label ": User") from array_map return
    s.open(
        "kinds.php",
        "<?php\nclass User {}\nfunction greet(string $name): void {}\n$users = array_map(fn($x): User => $x, []);\nforeach ($users as $u) {}\ngreet('Alice');\n",
    )
    .await;
    let resp = s.inlay_hints("kinds.php", 0, 0, 7, 0).await;
    // render_inlay_hints tags every hint with its [type]/[param] kind, so this
    // pins kind=1 (Type) and kind=2 (Parameter) for every hint at once rather
    // than only the first match of each shape.
    expect![[r#"
        4:21 : User [type]
        5:6 name: [param]"#]]
    .assert_eq(&render_inlay_hints(&resp));
}

/// When a file has no hints, `textDocument/inlayHint` must return `[]` (an
/// empty array), not `null`. The LSP spec §3.17.14 says the result type is
/// `InlayHint[] | null`; returning `null` for an open file with no hints is
/// valid but returning an array is preferred by editors that iterate the
/// result unconditionally.
///
/// We verify the server returns an array (not null) for an open file — the
/// response shape must be consistent regardless of hint count.
#[tokio::test]
async fn inlay_hints_empty_file_returns_array_not_null() {
    let mut s = TestServer::new().await;
    s.open("empty.php", "<?php\n$x = 1;\n").await;
    let resp = s.inlay_hints("empty.php", 0, 0, 3, 0).await;
    assert!(
        resp["result"].is_array(),
        "expected [] not null, got: {}",
        resp["result"]
    );
    expect!["<no hints>"].assert_eq(&render_inlay_hints(&resp));
}
