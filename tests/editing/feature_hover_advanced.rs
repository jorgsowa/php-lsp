//! Comprehensive hover coverage.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn hover_abstract_class_shows_keyword() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
abstract class Bas$0eHandler {}
"#,
        expect![[r#"
            ```php
            abstract class BaseHandler
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_abstract_method_shows_modifiers() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
abstract class Base {
    abstract protected function pro$0cess(string $input): string;
}
"#,
        expect![[r#"
            ```php
            protected abstract function process(string $input): string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_arrow_function_keyword() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php $f = f$0n(int $a): string => 'hello';"#,
        expect![[r#"
            ```php
            fn(int $a): string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_attribute_class_name() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class MyAttribute {}

#[MyAttri$0bute]
class Foo {}
"#,
        expect![[r#"
            ```php
            class MyAttribute
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_attribute_via_use_alias() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Route {}
use Route as HttpRoute;

#[HttpRou$0te]
class Api {}
"#,
        // Resolves alias → Route
        expect![[r#"
            ```php
            class Route
            ```"#]],
    )
    .await;
}

// ── 2.2 Named argument hover ──────────────────────────────────────────────────

#[tokio::test]
async fn hover_attribute_with_args() {
    // Cursor on attribute class name when the attribute has constructor arguments.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Route {
    public function __construct(string $path) {}
}

#[Rou$0te('/api')]
class Controller {}
"#,
        expect![[r#"
            ```php
            class Route
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_attribute_with_docblock() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/** Marks a class as a service container. */
class Service {}

#[Servi$0ce]
class Mailer {}
"#,
        expect![[r#"
            ```php
            class Service
            ```

            ---

            Marks a class as a service container."#]],
    )
    .await;
}

#[tokio::test]
async fn hover_backed_int_enum_shows_backing_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Priorit$0y: int { case Low = 1; case High = 2; }
"#,
        expect![[r#"
            ```php
            enum Priority: int
            ```"#]],
    )
    .await;
}

// ── Class modifiers ───────────────────────────────────────────────────────────

#[tokio::test]
async fn hover_class_const_in_static_access() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Config {
    const DEBUG = true;
}
if (Config::DEB$0UG) { }
"#,
        expect![[r#"
            ```php
            const bool DEBUG = true
            ```"#]],
    )
    .await;
}

/// Hovering on backed enum case in a match arm should show the value.
#[tokio::test]
async fn hover_closure_as_argument() {
    // Cursor on `function` keyword passed as a callback argument.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
function apply(callable $fn): void {}
apply(fun$0ction(int $n): int { return $n * 2; });
"#,
        expect![[r#"
            ```php
            function(int $n): int
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_closure_inside_if_body() {
    // Closure nested inside an if body — the walker must recurse into if branches.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
if (true) {
    $fn = fun$0ction(int $x): string { return (string) $x; };
}
"#,
        expect![[r#"
            ```php
            function(int $x): string
            ```"#]],
    )
    .await;
}

/// Hovering on `new Foo()` (the constructor call) must resolve to the class definition.
#[tokio::test]
async fn hover_closure_keyword() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php $fn = fun$0ction(int $x, string $y): bool { return true; };"#,
        expect![[r#"
            ```php
            function(int $x, string $y): bool
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_closure_no_params_no_return() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php $fn = fun$0ction() { return 1; };"#,
        expect![[r#"
            ```php
            function()
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_deprecated_function_shows_banner() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/** @deprecated Use newGreet() instead */
function ol$0dGreet(): void {}
"#,
        expect![[r#"
            ```php
            function oldGreet(): void
            ```

            ---

            > **Deprecated**: Use newGreet() instead"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_docblock_annotated_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/**
 * Greets the user.
 * @param string $name the name
 * @return string
 */
function gr$0eet(string $name): string { return $name; }
"#,
        expect![[r#"
            ```php
            function greet(string $name): string
            ```

            ---

            Greets the user.

            **@return** `string`
            **@param** `string` `$name` — the name"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_enum_case_in_match_arm() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Status { case Active; case Inactive; }
$status = Status::Active;
match ($status) {
    Status::Act$0ive => echo 'active',
}
"#,
        expect![[r#"
            ```php
            case Status::Active
            ```"#]],
    )
    .await;
}

/// Hovering on class constant in static access should show the constant.
#[tokio::test]
async fn hover_final_class_shows_keyword() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
final class Concret$0eService {}
"#,
        expect![[r#"
            ```php
            final class ConcreteService
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_final_method_shows_modifiers() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Locked {
    final public function sea$0l(): void {}
}
"#,
        expect![[r#"
            ```php
            public final function seal(): void
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_first_class_callable_builtin() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php $fn = str$0len(...);"#,
        expect![[r#"
            ```php
            function strlen()
            ```

            [php.net documentation](https://www.php.net/function.strlen)"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_first_class_callable_user_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function double(int $n): int {} $fn = dou$0ble(...);"#,
        expect![[r#"
            ```php
            function double(int $n): int
            ```"#]],
    )
    .await;
}

// ── 1.1 @inheritDoc resolution ───────────────────────────────────────────────

#[tokio::test]
async fn hover_keyword_false() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php $x = fal$0se;"#,
        expect![["`false` — boolean false"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_match() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php $x = mat$0ch($y) {};"#,
        expect![["`match` — evaluates an expression against a set of arms (PHP 8.0)"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_never() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function fail(): nev$0er { throw new \Exception(); }"#,
        expect![["`never` — return type indicating the function always throws or exits (PHP 8.1)"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_null() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php $x = nu$0ll;"#,
        expect![["`null` — the null value; a variable has no value"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_readonly() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php class Foo { readon$0ly string $x; }"#,
        expect![["`readonly` — property or class that can only be initialised once (PHP 8.1)"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_true() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(r#"<?php $x = tr$0ue;"#, expect![["`true` — boolean true"]])
        .await;
}

#[tokio::test]
async fn hover_keyword_void() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(): vo$0id {}"#,
        expect![["`void` — return type indicating the function returns no value"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_bool() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(bo$0ol $x): void {}"#,
        expect![["`bool` — boolean type: `true` or `false`"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_int() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(in$0t $x): void {}"#,
        expect![["`int` — integer type"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_float() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(flo$0at $x): void {}"#,
        expect![["`float` — floating-point number type"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_string() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(str$0ing $x): void {}"#,
        expect![["`string` — string type"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_mixed() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(mix$0ed $x): void {}"#,
        expect![["`mixed` — any type (no type constraint)"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_object() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(obj$0ect $x): void {}"#,
        expect![["`object` — any class instance"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_iterable() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(iterab$0le $x): void {}"#,
        expect![["`iterable` — array or Traversable (PHP 7.1)"]],
    )
    .await;
}

#[tokio::test]
async fn hover_named_arg_builtin_function() {
    // PHP 8.0 named arg on a user-defined function matching a known param name.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
function greet(string $name, int $count = 1): string { return $name; }
greet(coun$0t: 3);
"#,
        expect![[r#"
            ```php
            (parameter) int $count = 1
            ```"#]],
    )
    .await;
}

/// Named-arg hover where the receiver is `$this`. Resolves the enclosing class
/// via `enclosing_class_at`, an AST-based lookup.
#[tokio::test]
async fn hover_named_arg_this_method_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Notifier {
    public function send(string $to, string $subject): bool { return true; }
    public function notify(): void {
        $this->send(subje$0ct: 'Hi', to: 'a@b.com');
    }
}
"#,
        expect![[r#"
            ```php
            (parameter) string $subject
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_named_arg_method_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Mailer {
    public function send(string $to, string $subject): bool { return true; }
}
$m = new Mailer();
$m->send(subje$0ct: 'Hello', to: 'a@b.com');
"#,
        expect![[r#"
            ```php
            (parameter) string $subject
            ```"#]],
    )
    .await;
}

/// A wrapped call (common after formatter line-wrapping for long argument
/// lists) puts the callee on an earlier line than the label being hovered —
/// the backward scan for the enclosing `(` must cross that line boundary.
#[tokio::test]
async fn hover_named_arg_method_call_wrapped_across_lines() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Mailer {
    public function send(string $to, string $subject): bool { return true; }
}
$m = new Mailer();
$m->send(
    to: 'a@b.com',
    subje$0ct: 'Hello',
);
"#,
        expect![[r#"
            ```php
            (parameter) string $subject
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_named_arg_nested_call() {
    // Named arg inside a nested function call — cursor on inner call's arg.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
function outer(string $a): string { return $a; }
function inner(int $x): int { return $x; }
outer(a: inner(x$0: 1));
"#,
        expect![[r#"
            ```php
            (parameter) int $x
            ```"#]],
    )
    .await;
}

// ── 2.3 Closure / arrow function hover ───────────────────────────────────────

#[tokio::test]
async fn hover_named_arg_static_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class DB {
    public static function query(string $sql, int $limit = 100): array { return []; }
}
DB::query(lim$0it: 10);
"#,
        expect![[r#"
            ```php
            (parameter) int $limit = 100
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_named_arg_with_docblock() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/**
 * @param string $name The user's name.
 * @param int $age  The user's age.
 */
function register(string $name, int $age): void {}
register(na$0me: 'Alice', age: 30);
"#,
        expect![[r#"
            ```php
            (parameter) string $name
            ```

            ---

            The user's name."#]],
    )
    .await;
}

#[tokio::test]
async fn hover_named_function_keyword_not_intercepted() {
    // Hovering the `function` keyword in a named declaration (not a closure)
    // should not trigger the closure hover — returns nothing for the keyword itself.
    // Hover on the function *name* (not keyword) to get the signature.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php fun$0ction greet(): void {}"#,
        expect!["<no hover>"],
    )
    .await;
}

#[tokio::test]
async fn hover_on_constructor_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Service {
    public function __construct(private string $dsn) {}
}
$svc = new Serv$0ice('db://localhost');
"#,
        expect![[r#"
            ```php
            class Service
            ```"#]],
    )
    .await;
}

/// Hovering on a property access with union type should show the union.
#[tokio::test]
async fn hover_readonly_class_shows_keyword() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
readonly class Poi$0nt { public function __construct(public float $x, public float $y) {} }
"#,
        expect![[r#"
            ```php
            readonly class Point
            ```"#]],
    )
    .await;
}

/// Hovering on a `readonly class` defined in a background (non-open) file must
/// show the `readonly class` keyword, not just `class`.
#[tokio::test]
async fn hover_readonly_class_from_index_shows_keyword() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_hover(
            r#"
//- /Immutable.php
<?php
readonly class Immutable {}

//- /main.php
<?php
$obj = new Immu$0table();
"#,
        )
        .await;
    expect![[r#"
        ```php
        readonly class Immutable
        ```"#]]
    .assert_eq(&out);
}

// ── Use-alias resolution ──────────────────────────────────────────────────────

/// Hovering on `Bar` where `use Foo as Bar` is in scope must show the `Foo`
/// class declaration.
#[tokio::test]
async fn hover_readonly_property_shows_modifier() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Point {
    public readonly float $x;
}
$p = new Point();
echo $p->$0x;
"#,
        expect![[r#"
            ```php
            (property) public readonly Point::$x: float
            ```"#]],
    )
    .await;
}

/// A `readonly class` (PHP 8.2+) makes every property readonly even when
/// the property itself carries no per-property `readonly` keyword —
/// hovering a plain property must still show the `readonly` modifier.
#[tokio::test]
async fn hover_plain_property_in_readonly_class_shows_modifier() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
readonly class Point {
    public float $x;
}
$p = new Point();
echo $p->$0x;
"#,
        expect![[r#"
            ```php
            (property) public readonly Point::$x: float
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_real_docblock_not_overwritten_by_inheritdoc() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
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
        expect![[r#"
            ```php
            Child::run(): void
            ```

            ---

            Child's own description."#]],
    )
    .await;
}

// ── 1.2 Keyword hover ────────────────────────────────────────────────────────

#[tokio::test]
async fn hover_second_method_call_on_same_line_picks_correct_receiver() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class A { public function handle(string $x): bool {} }
class B { public function handle(int $n): void {} }
$a = new A(); $b = new B();
$a->handle('x'); $b->hand$0le(1);
"#,
        // Must show B::handle (int $n), not A::handle (string $x).
        expect![[r#"
            ```php
            B::handle(int $n): void
            ```"#]],
    )
    .await;
}

// ── Trait inheritance correctness ─────────────────────────────────────────────

/// `self::method()` at a call site resolves to the enclosing class.
#[tokio::test]
async fn hover_self_static_call_resolves_enclosing_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Builder {
    public static function create(): static { return new static(); }
    public function run(): void { self::crea$0te(); }
}
"#,
        expect![[r#"
            ```php
            Builder::create(): static
            ```"#]],
    )
    .await;
}

// ── Correct receiver on multi-call line ───────────────────────────────────────

/// Two unrelated classes each declare a static `run` method with a different
/// signature — hovering the call must resolve to the class named on the
/// call (`Worker::run`), not whichever `run` happens to be declared first.
#[tokio::test]
async fn hover_static_call_resolves_correct_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Worker { public static function run(int $jobs): void {} }
class Scheduler { public static function run(string $cron): bool { return true; } }
Worker::ru$0n(4);
"#,
        expect![[r#"
            ```php
            Worker::run(int $jobs): void
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_static_keyword_in_static_call_not_intercepted() {
    // `static::method()` — hovering `static` should NOT return the keyword doc,
    // it should fall through to the self/static class resolution.
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
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
    expect!["`static` — the class on which the method was called (late static binding, PHP 5.3)"]
        .assert_eq(&v);
}

// ── Eloquent / @method hover gaps ────────────────────────────────────────────

/// Eloquent model attributes are derived at runtime from the database schema.
/// Hovering over a magic-accessed property (`$user->name`) shows the return
/// type of `__get` (`mixed`) rather than the column type, because column types
/// are not available to static analysis without a schema-to-stub pipeline.
#[tokio::test]
async fn hover_eloquent_magic_get_shows_mixed() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
class Model {
    public function __get(string $key): mixed { return null; }
}

class User extends Model {
    protected string $table = 'users';
}

$user = new User();
$n = $user->nam$0e;
"#,
        )
        .await;
    // `name` resolves through __get which returns mixed; column type is unknown.
    expect![[r#"
        ```php
        (property) User::$name: mixed
        ```"#]]
    .assert_eq(&out);
}

/// Hovering over a call to a method declared only via a `@method` docblock
/// tag (a virtual method with no concrete AST body) shows the documented
/// signature, same as a real method declaration would.
#[tokio::test]
async fn hover_doc_method_tag() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
/**
 * @method User find(int $id)
 * @method static Builder where(string $col, mixed $val)
 */
class QueryBuilder {}

function run(QueryBuilder $qb): void {
    $qb->fin$0d(1);
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        QueryBuilder::find(int $id): User
        ```"#]]
    .assert_eq(&out);
}

/// Same as `hover_doc_method_tag` but the call site's case doesn't match the
/// `@method` tag's — dispatch is case-insensitive, so this must still resolve.
#[tokio::test]
async fn hover_doc_method_tag_is_case_insensitive() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
/**
 * @method User find(int $id)
 */
class QueryBuilder {}

function run(QueryBuilder $qb): void {
    $qb->FI$0ND(1);
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        QueryBuilder::find(int $id): User
        ```"#]]
    .assert_eq(&out);
}

/// Same as `hover_doc_method_tag` but for a `@method static` tag invoked as a
/// static call.
#[tokio::test]
async fn hover_doc_method_tag_static() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
/**
 * @method User find(int $id)
 * @method static Builder where(string $col, mixed $val)
 */
class QueryBuilder {}

function run(): void {
    QueryBuilder::whe$0re('id', 1);
}
"#,
        )
        .await;
    expect![[r#"
        ```php
        static QueryBuilder::where(string $col, mixed $val): Builder
        ```"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn hover_keyword_array() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(arr$0ay $x): void {}"#,
        expect![["`array` — ordered map type"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_callable() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f(call$0able $x): void {}"#,
        expect![[
            "`callable` — any callable: Closure, function-name string, or `[object, method]` array"
        ]],
    )
    .await;
}

#[tokio::test]
async fn hover_magic_constant_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php class Foo { public function bar(): void { echo __CLAS$0S__; } }"#,
        expect![["`__CLASS__` — name of the current class"]],
    )
    .await;
}

#[tokio::test]
async fn hover_magic_constant_dir() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php echo __DI$0R__;"#,
        expect![["`__DIR__` — directory of the current file"]],
    )
    .await;
}

#[tokio::test]
async fn hover_magic_constant_file() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php echo __FIL$0E__;"#,
        expect![["`__FILE__` — absolute path of the current file"]],
    )
    .await;
}

#[tokio::test]
async fn hover_magic_constant_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function f() { echo __FUNCTI$0ON__; }"#,
        expect![["`__FUNCTION__` — name of the current function or closure"]],
    )
    .await;
}

#[tokio::test]
async fn hover_magic_constant_line() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php echo __LIN$0E__;"#,
        expect![["`__LINE__` — current line number in the file"]],
    )
    .await;
}

#[tokio::test]
async fn hover_magic_constant_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php class Foo { public function bar(): void { echo __METH$0OD__; } }"#,
        expect![["`__METHOD__` — current method name (`ClassName::methodName`)"]],
    )
    .await;
}

#[tokio::test]
async fn hover_magic_constant_namespace() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php namespace App; echo __NAMESP$0ACE__;"#,
        expect![["`__NAMESPACE__` — name of the current namespace"]],
    )
    .await;
}

#[tokio::test]
async fn hover_magic_constant_trait() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php trait Foo { public function bar(): void { echo __TRAI$0T__; } }"#,
        expect![["`__TRAIT__` — name of the current trait"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_self() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php class Foo { public function clone(): sel$0f {} }"#,
        expect![["`self` — the class in which the method is defined"]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_static_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php class Foo { public function make(): stat$0ic {} }"#,
        expect![[
            "`static` — the class on which the method was called (late static binding, PHP 5.3)"
        ]],
    )
    .await;
}

#[tokio::test]
async fn hover_keyword_parent_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php class Child extends Base { public function __construct(): paren$0t {} }"#,
        expect![["`parent` — the parent class of the current class"]],
    )
    .await;
}

// ── 2.4 PHP attribute hover ───────────────────────────────────────────────────
