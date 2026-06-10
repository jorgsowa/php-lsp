use super::*;

use expect_test::expect;

#[tokio::test]
async fn signature_help_at_first_arg() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
function greet(string $name, int $count = 1): string { return $name; }
greet($0);
"#,
        )
        .await;
    expect!["▶ greet(string $name, int $count = 1)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_at_second_arg() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
function greet(string $name, int $count = 1): string { return $name; }
greet('x', $0);
"#,
        )
        .await;
    expect!["▶ greet(string $name, int $count = 1)  @param1"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_for_method_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
class Greeter {
    public function hello(string $name): string { return $name; }
}
$g = new Greeter();
$g->hello($0);
"#,
        )
        .await;
    expect!["▶ hello(string $name)  @param0"].assert_eq(&out);
}

/// Cursor inside the inner call of `outer(inner($0), 2)` must show `inner`'s
/// signature, not `outer`'s. A parser that tracks only one call frame will
/// show `outer` here — this test catches that regression.
#[tokio::test]
async fn signature_help_nested_call_shows_inner_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
function inner(int $x): int { return $x; }
function outer(int $a, int $b): void {}
outer(inner($0), 2);
"#,
        )
        .await;
    expect!["▶ inner(int $x)  @param0"].assert_eq(&out);
}

/// Calling a function with variadic params and multiple args: the active
/// parameter must stay pinned to the variadic param regardless of arg count.
#[tokio::test]
async fn signature_help_variadic_stays_active_past_first_arg() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
function sum(int ...$vals): int { return array_sum($vals); }
sum(1, 2, $0);
"#,
        )
        .await;
    expect!["▶ sum(int ...$vals)  @param0"].assert_eq(&out);
}

/// Signature help for a static method call `Cls::method($0)` must resolve to
/// that class's method, not fall back to a global function with the same name.
#[tokio::test]
async fn signature_help_static_method_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
class Math {
    public static function add(int $a, int $b): int { return $a + $b; }
}
Math::add($0);
"#,
        )
        .await;
    expect!["▶ add(int $a, int $b)  @param0"].assert_eq(&out);
}

/// Signature help for a zero-parameter function must not crash and must not
/// expose a stale `activeParameter` from a previous call in the same file.
#[tokio::test]
async fn signature_help_zero_param_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
function ping(): bool { return true; }
ping($0);
"#,
        )
        .await;
    expect!["▶ ping()"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_outside_call_returns_no_signature() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
function greet(string $name): string { return $name; }
$x = 1$0;
"#,
        )
        .await;
    expect!["<no signature>"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_unknown_function_returns_no_signature() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
unknown($0);
"#,
        )
        .await;
    expect!["<no signature>"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_builtin_strlen() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
strlen($0);
"#,
        )
        .await;
    expect!["▶ strlen($string)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_default_param_values_shown() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
function greet(string $name = 'World', int $count = 1): string { return $name; }
greet($0);
"#,
        )
        .await;
    expect!["▶ greet(string $name = 'World', int $count = 1)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_nested_call_outer() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
function inner(int $x): int { return $x; }
function outer(int $a, int $b): void {}
outer(inner(1), $0);
"#,
        )
        .await;
    expect!["▶ outer(int $a, int $b)  @param1"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_trait_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
trait Logger {
    public function log(string $msg, int $level): void {}
}
log($0);
"#,
        )
        .await;
    expect!["▶ log(string $msg, int $level)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_enum_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
enum Status {
    public static function from(string $value): self {}
}
from($0);
"#,
        )
        .await;
    expect!["▶ from(string $value)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_param_doc_from_docblock() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
/**
 * @param string $name The user's name
 * @param int $times How many times to greet
 */
function greet(string $name, int $times): void {}
greet($0);
"#,
        )
        .await;
    expect!["▶ greet(string $name, int $times)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_interface_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
interface Logger {
    public function log(string $msg, int $level): void;
}
log($0);
"#,
        )
        .await;
    expect!["▶ log(string $msg, int $level)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_fqn_builtin_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
\strlen($0);
"#,
        )
        .await;
    expect!["▶ strlen($string)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_constructor_new_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
class Greeter {
    public function __construct(string $name, int $times = 1) {}
}
new Greeter($0);
"#,
        )
        .await;
    expect!["▶ Greeter(string $name, int $times = 1)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn signature_help_builtin_variadic_sprintf() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
sprintf('%d %s %d', 1, 'x', $0);
"#,
        )
        .await;
    expect!["▶ sprintf($format, ...$values)  @param1"].assert_eq(&out);
}

/// Function defined in a separate file; signature must resolve via workspace index.
#[tokio::test]
async fn signature_help_cross_file_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"
//- /helpers.php
<?php
function compute(string $name, int $count): int { return 0; }

//- /main.php
<?php
compute($0);
"#,
        )
        .await;
    expect!["▶ compute(string $name, int $count)  @param0"].assert_eq(&out);
}

/// Fully-qualified name call to a function in another file/namespace.
#[tokio::test]
async fn signature_help_cross_file_fqn() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"
//- /lib.php
<?php
namespace App\Lib;
function transform(array $data, bool $strict): string { return ''; }

//- /main.php
<?php
\App\Lib\transform($0);
"#,
        )
        .await;
    expect!["▶ App\\Lib\\transform(array $data, bool $strict)  @param0"].assert_eq(&out);
}

/// Cross-file method call: `$obj->method(` must show the signature from the
/// class that `$obj` is typed as, not a different class with the same method name.
#[tokio::test]
async fn signature_help_cross_file_method_picks_receiver_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"
//- /src/Alpha.php
<?php
class Alpha {
    public function process(string $label, int $limit): void {}
}

//- /src/Beta.php
<?php
class Beta {
    public function process(int $id): void {}
}

//- /main.php
<?php
$b = new Beta();
$b->process($0);
"#,
        )
        .await;
    // Beta::process has one param ($id), not Alpha::process's two params
    expect!["▶ process(int $id)  @param0"].assert_eq(&out);
}

/// Inherited method: cursor inside `$this->method(` inside a subclass body
/// must show the signature from the parent class where the method is defined.
#[tokio::test]
async fn signature_help_this_method_walks_inheritance_chain() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"
//- /src/Base.php
<?php
class Base {
    public function render(string $template, array $vars = []): string { return ''; }
}

//- /main.php
<?php
class Page extends Base {
    public function show(): void {
        $this->render($0);
    }
}
"#,
        )
        .await;
    // FileIndex stores has_default:bool, not the actual default value text.
    expect!["▶ render(string $template, array $vars = ...)  @param0"].assert_eq(&out);
}

/// Static dispatch: `ClassName::method(` must show the signature for that class.
#[tokio::test]
async fn signature_help_static_dispatch_named_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"
//- /src/Formatter.php
<?php
class Formatter {
    public static function format(string $template, mixed ...$args): string {}
}

//- /main.php
<?php
Formatter::format($0);
"#,
        )
        .await;
    expect!["▶ format(string $template, mixed ...$args)  @param0"].assert_eq(&out);
}

/// parent:: dispatch: inside a subclass method body `parent::method(` must show
/// the signature from the actual parent class, not the subclass.
#[tokio::test]
async fn signature_help_parent_static_dispatch() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"
//- /src/Base.php
<?php
class Base {
    public function boot(string $env, bool $debug = false): void {}
}

//- /main.php
<?php
class App extends Base {
    public function boot(string $env, bool $debug = false): void {
        parent::boot($0);
    }
}
"#,
        )
        .await;
    expect!["▶ boot(string $env, bool $debug = ...)  @param0"].assert_eq(&out);
}
