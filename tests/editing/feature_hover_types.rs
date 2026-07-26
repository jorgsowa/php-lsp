//! Comprehensive hover coverage.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn hover_backed_enum_case_in_match_arm() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Priority: int { case Low = 1; case High = 2; }
match ($p) {
    Priority::H$0igh => echo 'urgent',
}
"#,
        expect![[r#"
            ```php
            case Priority::High = 2
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_backed_enum_shows_backing_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Stat$0us: string { case Active = 'active'; }
"#,
        expect![[r#"
            ```php
            enum Status: string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_class_constant() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Config {
    const VERSI$0ON = 42;
}
"#,
        expect![[r#"
            ```php
            const int VERSION = 42
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_enum_case_declaration() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Status { case Acti$0ve; case Inactive; }
"#,
        expect![[r#"
            ```php
            case Status::Active
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_function_with_signature() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php function gr$0eet(string $name, int $count = 1): string {}"#,
        expect![[r#"
            ```php
            function greet(string $name, int $count = 1): string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_nullable_param_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
function sho$0w(?string $label): void {}
"#,
        expect![[r#"
            ```php
            function show(?string $label): void
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_property_access() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class User {
    public string $name = '';
}
$u = new User();
echo $u->na$0me;
"#,
        expect![[r#"
            ```php
            (property) public User::$name: string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_static_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Config {
    public static string $version = '1.0';
}
Config::$ver$0sion;
"#,
        expect![[r#"
            ```php
            (property) public static Config::$version: string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_template_at_call_site_shows_literal_t() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/** @template T @param T $x @return T */
function identity($x) { return $x; }
$myString = 'hello';
// Hovering on the return value assignment
$result = identi$0ty($myString);
"#,
        expect![[r#"
            ```php
            function identity($x)
            ```

            ---

            **@return** `T`
            **@param** `T` `$x`
            **@template** `T`"#]],
    )
    .await;
}
#[tokio::test]
async fn hover_template_param_type_in_signature() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/** @template T @param T $v @return T */
function box($v) { }
$result = box$0('hello');
"#,
        expect![[r#"
            ```php
            function box($v)
            ```

            ---

            **@return** `T`
            **@param** `T` `$v`
            **@template** `T`"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_union_type_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Config {
    public string|int $setting = '';
}
$c = new Config();
echo $c->se$0tting;
"#,
        expect![[r#"
            ```php
            (property) public Config::$setting: string|int
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_union_typed_variable_shows_union() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Cat {}
class Dog {}
function pet(Cat|Dog $a): void { $a$0; }
"#,
        expect![[r#"
            `$a` `Cat|Dog`"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_variable_from_enum_case_shows_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Status { case Active; case Inactive; }
$s = Status::Active;
$s$0;
"#,
        expect![[r#"`$s` `Status`"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_property_shows_docblock() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class User {
    /** The user's display name. */
    public string $name = '';
}
$u = new User();
echo $u->na$0me;
"#,
        expect![[r#"
            ```php
            (property) public User::$name: string
            ```

            ---

            The user's display name."#]],
    )
    .await;
}

#[tokio::test]
async fn hover_property_with_var_tag_shows_type_annotation() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class User {
    /** @var string */
    public $name = '';
}
$u = new User();
echo $u->na$0me;
"#,
        expect![[r#"
            ```php
            (property) public User::$name: string
            ```

            ---

            **@var** `string`"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_property_with_var_tag_and_description() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class User {
    /** @var string The display name. */
    public $name = '';
}
$u = new User();
echo $u->na$0me;
"#,
        expect![[r#"
            ```php
            (property) public User::$name: string
            ```

            ---

            **@var** `string` — The display name."#]],
    )
    .await;
}

#[tokio::test]
async fn hover_this_property_shows_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Counter {
    public int $count = 0;
    public function increment(): void {
        $this->co$0unt;
    }
}
"#,
        expect![[r#"
            ```php
            (property) public Counter::$count: int
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_nullsafe_property_shows_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Profile { public string $bio = ''; }
$p = new Profile();
$p?->bi$0o;
"#,
        // mir 0.54.0: the declared property type, no blanket `|null` widening —
        // `$p` is provably non-null here.
        expect![[r#"
            ```php
            (property) public Profile::$bio: string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_promoted_property_shows_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Point {
    public function __construct(
        public float $x,
        public float $y,
    ) {}
}
$p = new Point(1.0, 2.0);
$p->$0x;
"#,
        expect![[r#"
            ```php
            (property) public Point::$x: float
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_readonly_promoted_property_shows_readonly_modifier() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Point {
    public function __construct(
        public readonly float $x,
    ) {}
}
$p = new Point(1.0);
$p->$0x;
"#,
        expect![[r#"
            ```php
            (property) public readonly Point::$x: float
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_promoted_property_shows_only_its_param_docblock() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class User {
    /**
     * Create a user.
     * @param string $name The user's display name
     * @param int $age The user's age
     * @return void
     * @throws \InvalidArgumentException
     */
    public function __construct(
        public string $name,
        public int $age,
    ) {}
}
$u = new User('Alice', 30);
$u->na$0me;
"#,
        expect![[r#"
            ```php
            (property) public User::$name: string
            ```

            ---

            **@param** `string` `$name` — The user's display name"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_promoted_property_with_no_matching_param_docblock() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class User {
    /**
     * Create a user.
     * @return void
     */
    public function __construct(
        public string $name,
    ) {}
}
$u = new User('Alice');
$u->na$0me;
"#,
        expect![[r#"
            ```php
            (property) public User::$name: string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_catch_variable_shows_exception_class() {
    let mut s = TestServer::new().await;
    s.check_hover_annotated(
        r#"<?php
class DatabaseException { public function getQuery(): string {} }
try {
    doWork();
} catch (DatabaseException $e) {
    $e$0;
}
"#,
        expect![[r#"`$e` `DatabaseException`"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_method_call_shows_declaring_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Mailer { public function process(string $to): bool {} }
class Queue  { public function process(int $id): void {} }
$mailer = new Mailer();
$mailer->proc$0ess();
"#,
        expect![[r#"
            ```php
            Mailer::process(string $to): bool
            ```"#]],
    )
    .await;
}

/// Enum that implements an interface should show the `implements` clause in hover.
#[tokio::test]
async fn hover_enum_with_implements_shows_interface() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
enum Stat$0us: string implements \Stringable {}
"#,
        expect![[r#"
            ```php
            enum Status: string implements \Stringable
            ```"#]],
    )
    .await;
}

// ── __get magic property access ───────────────────────────────────────────────

/// Accessing a property through `__get` shows the return type in hover even
/// when no declared PHP property exists.
#[tokio::test]
async fn magic_get_hover_shows_return_type() {
    let mut s = TestServer::new().await;
    let out = s
        .check_hover(
            r#"<?php
class DynamicModel {
    private array $data = [];
    public function __get(string $name): mixed { return $this->data[$name] ?? null; }
}

$m = new DynamicModel();
$v = $m->nam$0e;
"#,
        )
        .await;
    expect![[r#"
        ```php
        (property) DynamicModel::$name: mixed
        ```"#]]
    .assert_eq(&out);
}

// ── Type system — generics, templates, and callable types ─────────────────────

/// `@template T of \Countable` — the constraint is surfaced in hover.
#[tokio::test]
async fn hover_template_bound_shows_constraint() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
/**
 * @template T of \Countable
 * @param T $items
 * @return T
 */
function proce$0ss($items) { return $items; }
"#,
        expect![[r#"
            ```php
            function process($items)
            ```

            ---

            **@return** `T`
            **@param** `T` `$items`
            **@template** `T` of `\Countable`"#]],
    )
    .await;
}

/// `$x = null; $x = new Foo()` — hover resolves `$x` to `Foo` at the final assignment.
#[tokio::test]
async fn hover_reassignment_after_null_resolves_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Foo { public function doFoo(): void {} }
$x = null;
$x = new Foo();
$x$0;
"#,
        expect![[r#"`$x` `Foo`"#]],
    )
    .await;
}

/// `$fn = strlen(...)` — first-class callable, typed by mir with the target's
/// full signature.
#[tokio::test]
async fn hover_first_class_callable_typed_as_closure() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
$fn = strlen(...);
$fn$0;
"#,
        expect!["`$fn` `Closure(string): int<0, max>`"],
    )
    .await;
}

/// `$fn = $obj->handle(...)` — method first-class callable; mir resolves the
/// closure to the target method's full signature.
#[tokio::test]
async fn hover_method_first_class_callable_typed_as_closure() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Svc {
    public function handle(string $name, int $count): bool { return true; }
}
$obj = new Svc();
$fn = $obj->handle(...);
$fn$0;
"#,
        expect!["`$fn` `Closure(string, int): bool`"],
    )
    .await;
}
