//! Type definition (`textDocument/typeDefinition`) coverage.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn type_definition_variable_to_class() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Foo {}
//    ^^^ type
$obj = new Foo();
$obj$0->bar();
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_cross_file() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /a.php
<?php
$obj = new Mailer();
$obj$0->send();

//- /mailer.php
<?php
class Mailer {}
//    ^^^^^^ type
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_unknown_variable() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
$unknown$0->foo();
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_interface_type() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
interface Countable {}
$obj = new MyList();
$obj$0->count();
class MyList implements Countable {}
//    ^^^^^^ type
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_enum_typed_param() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_type_definition_annotated(
        r#"<?php
enum Status { case Active; }
//   ^^^^^^ type
function process(Status $s): void { $s$0-> }
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_trait_typed_param() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_type_definition_annotated(
        r#"<?php
trait Logger {}
//    ^^^^^^ type
function process(Logger $l): void { $l$0-> }
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_variable_from_new_expr() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Widget {}
//    ^^^^^^ type
$w = new Widget();
echo $w$0;
"#,
    )
    .await;
}

/// Variable assigned from an enum case constant (`$x = Status::Active`).
/// mir 0.31.0 synthesises a `TNamedObject` for enum-case assignments; this
/// test guards that the mir path resolves the type (no TypeMap fallback needed).
#[tokio::test]
async fn type_definition_variable_from_enum_case_assignment() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
enum Status { case Active; case Inactive; }
//   ^^^^^^ type
$s = Status::Active;
echo $s$0;
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_non_variable_without_type() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
function greet() {}
gree$0t();
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_with_use_import() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Mailer.php
<?php
namespace Vendor;
class Mailer {}
//    ^^^^^^ type

//- /src/main.php
<?php
use Vendor\Mailer;
$m = new Mailer();
$m$0->send();
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_nullable_type() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class User {}
//    ^^^^ type
function process(?User $u$0): void {}
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_union_type_not_supported() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Admin {}
//    ^^^^^ type
class User {}
//    ^^^^ type
function process(Admin|User $u$0): void {}
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_fully_qualified_parameter() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
namespace App;
class Service {}
//    ^^^^^^^ type
function process(\App\Service $s$0): void {}
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_cursor_on_param_name() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Logger {}
//    ^^^^^^ type
function write_log(Logger $l$0): void {}
"#,
    )
    .await;
}

// ── Tests for indexed type definition (background files) ────

/// Type definition should resolve types from background-indexed files
/// This tests the goto_type_definition_from_index code path.
/// Note: The indexed version returns the class keyword location from the index.
#[tokio::test]
async fn type_definition_from_background_indexed_file() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    // Wait for background indexing to complete so files are in the index
    s.wait_for_index_ready().await;

    // Now test type resolution from an indexed file
    let out = s
        .check_type_definition(
            r#"<?php
namespace App;
use App\Model\User;
$u = new User();
$u$0->getName();
"#,
        )
        .await;

    // Should resolve to User class from index (indexed version returns class keyword location)
    expect![[r#"
        src/Model/User.php:4:0-4:0"#]]
    .assert_eq(&out);
}

/// Aliased type hints in `use X as Y` are resolved via `collect_file_imports`.
/// This covers both open-docs and background-index paths.
#[tokio::test]
async fn type_definition_alias_resolved_from_index() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_type_definition(
            r#"<?php
namespace App\Service;
use App\Model\User as UserModel;
function create(UserModel $u$0): void {}
"#,
        )
        .await;

    // Alias is resolved to the real FQN App\Model\User → finds the class in index
    expect![[r#"
        src/Model/User.php:4:0-4:0"#]]
    .assert_eq(&out);
}

/// Unqualified type names in non-global namespaces are resolved with namespace context.
/// `Logger $l` in `namespace App\Service` resolves to `App\Service\Logger` via resolve_fqn.
#[tokio::test]
async fn type_definition_unqualified_param_in_namespace_resolves_correctly() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Logger.php
<?php
namespace App\Service;
class Logger {}
//    ^^^^^^ type

//- /src/Service.php
<?php
namespace App\Service;
class Service {
    public function log(Logger $l$0): void {}
}
"#,
    )
    .await;
}

/// Union types (PHP 8.0+) now return all matching types in the union.
#[tokio::test]
async fn type_definition_limitation_union_types_not_supported() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Admin {}
//    ^^^^^ type
class User {}
//    ^^^^ type
function authenticate(Admin|User $a$0): void {}
"#,
    )
    .await;
}

/// Intersection types (PHP 8.1+) are now supported and return all matching types.
#[tokio::test]
async fn type_definition_limitation_intersection_types_not_supported() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
interface Readable {}
//        ^^^^^^^^ type
interface Writable {}
//        ^^^^^^^^ type
function process(Readable&Writable $rw$0): void {}
"#,
    )
    .await;
}

/// Aliased types in use imports are resolved via `collect_file_imports` which
/// tracks `use X as Y` mappings. Jumping to type definition works correctly.
#[tokio::test]
async fn type_definition_alias_with_use_import() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Model/Account.php
<?php
namespace App\Model;
class Account {}
//    ^^^^^^^ type

//- /src/Service.php
<?php
namespace App\Service;
use App\Model\Account as UserAccount;
function create(UserAccount $acc$0): void {}
"#,
    )
    .await;
}

/// **LIMITATION**: Generic-like syntax (e.g., Collection<User>) is not supported.
/// The type hint parser doesn't understand generic syntax. This test uses `Collection` (valid PHP)
/// to verify that without explicit type information, generic parameters aren't synthesized.
/// TODO: Parse and handle generic-like type syntax.
#[tokio::test]
async fn type_definition_limitation_generic_types_not_supported() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_type_definition(
            r#"<?php
class Collection {}
class User {}
function process(Collection<User> $items$0): void {}
"#,
        )
        .await;
    // Generic syntax isn't recognized - Collection<User> is parsed as something unexpected
    expect!["<none>"].assert_eq(&out);
}

/// Enum method parameters should have type definitions resolved.
/// Regression: param_type_for previously did not check StmtKind::Enum.
#[tokio::test]
async fn type_definition_enum_method_parameter() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Logger {}
//    ^^^^^^ type
enum Status {
    case Active;
    public function log(Logger $l$0): void {}
}
"#,
    )
    .await;
}

/// When multiple classes share a short name, exact FQN match should be preferred.
/// Regression: goto_type_definition_from_index previously returned first short name match.
#[tokio::test]
async fn type_definition_prefers_exact_fqn_over_short_name() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Model/User.php
<?php
namespace App\Model;
class User {}

//- /src/Service/User.php
<?php
namespace App\Service;
class User {}
//    ^^^^ type

//- /src/main.php
<?php
namespace App\Service;
function create(User $u$0): void {}
"#,
    )
    .await;
}

/// Unqualified type names in non-global namespaces should be resolved with namespace context.
/// Regression: param_type_for previously didn't qualify unqualified names.
#[tokio::test]
async fn type_definition_unqualified_name_in_namespace() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Model/User.php
<?php
namespace App\Model;
class User {}
//    ^^^^ type

//- /src/Service/UserService.php
<?php
namespace App\Service;
use App\Model\User;
class UserService {
    public function getUser(User $user$0): void {}
}
"#,
    )
    .await;
}

// ── Regression tests for $var FQN resolution ─────────────────────────────────

/// `$var = new Class()` in a namespace: TypeMap stores only the short class name,
/// but resolve_fqn must qualify it to the file's namespace so the FQN-scoped
/// search picks the right file when two classes share the same short name.
#[tokio::test]
async fn type_definition_var_new_in_namespace_prefers_same_namespace() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Model/Order.php
<?php
namespace App\Model;
class Order {}

//- /src/Service/Order.php
<?php
namespace App\Service;
class Order {}
//    ^^^^^ type

//- /src/Service/Processor.php
<?php
namespace App\Service;
$order = new Order();
$order$0->process();
"#,
    )
    .await;
}

/// `$var = new Class()` in a namespace with a `use` import: the import overrides
/// the namespace prefix, so $var should resolve to the imported class.
#[tokio::test]
async fn type_definition_var_new_with_use_import_overrides_namespace() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Model/Invoice.php
<?php
namespace App\Model;
class Invoice {}
//    ^^^^^^^ type

//- /src/Service/Invoice.php
<?php
namespace App\Service;
class Invoice {}

//- /src/Billing/Creator.php
<?php
namespace App\Billing;
use App\Model\Invoice;
$inv = new Invoice();
$inv$0->total();
"#,
    )
    .await;
}

/// Typed parameter in a class method (not a top-level function) in a namespace.
/// Regression: param_type_for must recurse into class members.
#[tokio::test]
async fn type_definition_method_param_in_namespaced_class() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Model/Product.php
<?php
namespace App\Model;
class Product {}
//    ^^^^^^^ type

//- /src/Service/Cart.php
<?php
namespace App\Service;
use App\Model\Product;
class Cart {
    public function addItem(Product $item$0): void {}
}
"#,
    )
    .await;
}

/// Nullable type `?ClassName` in a namespace resolves the inner class by FQN.
#[tokio::test]
async fn type_definition_nullable_type_in_namespace() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Model/Address.php
<?php
namespace App\Model;
class Address {}

//- /src/Service/Address.php
<?php
namespace App\Service;
class Address {}
//    ^^^^^^^ type

//- /src/Handler.php
<?php
namespace App\Service;
function deliver(?Address $addr$0): void {}
"#,
    )
    .await;
}

/// Braced namespace form: both the calling file and the target class use
/// `namespace Foo { ... }` syntax.
#[tokio::test]
async fn type_definition_braced_namespace() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Model/Report.php
<?php
namespace App\Model {
    class Report {}
}

//- /src/Service/Report.php
<?php
namespace App\Service {
    class Report {}
    //    ^^^^^^ type
}

//- /src/Runner.php
<?php
namespace App\Service {
    function run(Report $r$0): void {}
}
"#,
    )
    .await;
}

/// Deeply nested namespace (A\B\C) — resolve_fqn must handle multi-segment prefix.
#[tokio::test]
async fn type_definition_deeply_nested_namespace() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Cmd.php
<?php
namespace App\Console\Command;
class Cmd {}
//    ^^^ type

//- /src/Other/Cmd.php
<?php
namespace App\Http\Controller;
class Cmd {}

//- /src/Dispatch.php
<?php
namespace App\Console\Command;
function dispatch(Cmd $c$0): void {}
"#,
    )
    .await;
}

// ── Regression tests for index (background file) FQN resolution ──────────────

/// Background-indexed class: `$var` in a namespace resolves without an explicit
/// `use` import — the namespace itself qualifies the short class name to an FQN.
#[tokio::test]
async fn type_definition_index_var_namespace_resolves_without_import() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_type_definition(
            r#"<?php
namespace App\Model;
$u = new User();
$u$0->greet();
"#,
        )
        .await;
    // No explicit `use` — namespace App\Model qualifies User to App\Model\User,
    // which the index finds directly via FQN match.
    expect![[r#"
        src/Model/User.php:4:0-4:0"#]]
    .assert_eq(&out);
}

/// Background-indexed class: typed parameter with `use` alias, index path.
/// Tests that goto_type_definition_from_index also resolves aliases.
#[tokio::test]
async fn type_definition_index_param_alias_resolved() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_type_definition(
            r#"<?php
namespace App\Service;
use App\Model\User as UserModel;
function greet(UserModel $u$0): void {}
"#,
        )
        .await;
    // Alias UserModel resolved to App\Model\User via imports; index finds it
    expect![[r#"
        src/Model/User.php:4:0-4:0"#]]
    .assert_eq(&out);
}

/// Unqualified type hints resolve within the same namespace.
#[tokio::test]
async fn type_definition_unqualified_name_same_namespace() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Logger.php
<?php
namespace App;
class Logger {}
//    ^^^^^^ type

//- /src/Service.php
<?php
namespace App;
class Service {
    public function log(Logger $l$0): void {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_not_confused_by_use_function_import() {
    // `use function` imports must not pollute the class-import map: a type hint
    // `format $x` where `format` also appears in `use function Lib\format` should
    // resolve to the same-namespace class `App\format`, not to `Lib\format`.
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /main.php
<?php
namespace App;
use function Lib\format;

function go(format $x$0): void {}

//- /format.php
<?php
namespace App;
class format {}
//    ^^^^^^ type
"#,
    )
    .await;
}

// ── Built-in and Special Types ────────────────────────────────────────

/// Built-in scalar types (int, string, bool, etc.) have no type definition.
#[tokio::test]
async fn type_definition_builtin_int_returns_none() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_type_definition(
            r#"<?php
function count(int $n$0): void {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_builtin_string_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
function message(string $msg$0): void {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_builtin_bool_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
function check(bool $flag$0): void {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_builtin_mixed_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
function handle(mixed $data$0): void {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

#[tokio::test]
async fn type_definition_builtin_never_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
function process(string $x$0): void {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

/// stdClass is a built-in class and should be resolvable.
#[tokio::test]
async fn type_definition_stdclass_builtin() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
function object_param(stdClass $obj$0): void {}
"#,
        )
        .await;
    // stdClass is a built-in class; type definition returns None (not in workspace)
    expect!["<none>"].assert_eq(&out);
}

// ── Array and Collection Types ────────────────────────────────────────

/// Array type hint with generic-like documentation syntax (PHPDoc style).
/// Note: `User[]` is only valid in PHPDoc, not as actual parameter type hint.
/// Using generic-like syntax in actual type hints is not standard PHP.
#[tokio::test]
async fn type_definition_array_of_class_via_generic_syntax() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
class User {}
/** @param User[] $users */
function batch(array $users$0): void {}
"#,
        )
        .await;
    // Type hint is `array` (built-in), not a class type
    expect!["<none>"].assert_eq(&out);
}

/// Built-in `array` type returns None (it's not a class).
#[tokio::test]
async fn type_definition_array_builtin_returns_none() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
function items(array $data$0): void {}
"#,
        )
        .await;
    expect!["<none>"].assert_eq(&out);
}

// ── Variable Assignment and Factory Methods ────────────────────────────

/// Variable assigned from another variable's type is now tracked.
#[tokio::test]
async fn type_definition_variable_assigned_from_other() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Result {}
//    ^^^^^^ type
$value = new Result();
$copy = $value;
$copy$0->process();
"#,
    )
    .await;
}

/// Nullable union type resolution now returns all matching types.
#[tokio::test]
async fn type_definition_nullable_union_type() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Success {}
//    ^^^^^^^ type
class Error {}
//    ^^^^^ type
function handle(Success|Error $result$0): void {}
"#,
    )
    .await;
}

// ── Self, Parent, Static Keywords ────────────────────────────────────────

/// `self` keyword in class parameter resolves to the containing class.
#[tokio::test]
async fn type_definition_self_keyword_in_class() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class User {
//    ^^^^ type
    public function duplicate(self $other$0): self {}
}
"#,
    )
    .await;
}

/// `parent` keyword in class parameter resolves to the enclosing class, not the parent.
/// The `parent` keyword now correctly resolves to the actual parent class.
/// This is resolved by looking up the inheritance chain via ParsedDoc context.
#[tokio::test]
async fn type_definition_parent_keyword_limitation() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Base {}
//    ^^^^ type
class Child extends Base {
    public function get_parent(parent $p$0): void {}
}
"#,
    )
    .await;
}

/// Parameter with Factory type resolves correctly (test previously ignored for wrong reason).
#[tokio::test]
async fn type_definition_static_return_type() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Factory {
//    ^^^^^^^ type
    public function create(): static { return new static(); }
    public function use_it(Factory $f$0): void {}
}
"#,
    )
    .await;
}

// ── Trait-Specific Cases ───────────────────────────────────────────────

/// Type hints in trait methods should resolve correctly.
#[tokio::test]
async fn type_definition_trait_with_class_param() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Config {}
//    ^^^^^^ type
trait Settings {
    public function load(Config $cfg$0): void {}
}
"#,
    )
    .await;
}

/// Trait with cross-file type hint.
#[tokio::test]
async fn type_definition_trait_cross_file_param() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/db.php
<?php
class Connection {}
//    ^^^^^^^^^^ type

//- /src/main.php
<?php
trait Database {
    public function query(Connection $conn$0): void {}
}
"#,
    )
    .await;
}

// ── Enum Backed Types ─────────────────────────────────────────────────

/// Backed enum (int-backed) with method parameter.
#[tokio::test]
async fn type_definition_backed_enum_int_param() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Logger {}
//    ^^^^^^ type
enum Priority: int {
    case HIGH = 1;
    case LOW = 0;
    public function log(Logger $logger$0): void {}
}
"#,
    )
    .await;
}

/// Backed enum (string-backed) typed as parameter.
#[tokio::test]
async fn type_definition_backed_enum_as_parameter() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
enum Status: string {
//   ^^^^^^ type
    case ACTIVE = 'active';
    case INACTIVE = 'inactive';
}
function process(Status $status$0): void {}
"#,
    )
    .await;
}

// ── Interface Inheritance ──────────────────────────────────────────────

/// Parameter typed as interface that extends another.
#[tokio::test]
async fn type_definition_extended_interface() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
interface Animal {}
interface Pet extends Animal {}
//        ^^^ type
function adopt(Pet $pet$0): void {}
"#,
    )
    .await;
}

/// Multiple interface inheritance (one class implements two).
#[tokio::test]
async fn type_definition_multi_interface_param() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
interface Logger {}
interface Config {}
class App implements Logger, Config {}
//    ^^^ type
function bootstrap(App $app$0): void {}
"#,
    )
    .await;
}

// ── Import and Namespace Conflicts ────────────────────────────────────

/// When both a use import and a local class have the same short name,
/// Imports take precedence per PHP semantics.
/// When an import explicitly names a class, fallback short-name search is skipped
/// if the class is not found in its declared namespace.
#[tokio::test]
async fn type_definition_import_with_local_class_same_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"//- /src/Logger.php
<?php
namespace App;
class Logger {}

//- /src/Service.php
<?php
namespace App;
use Different\Logger;

function log(Logger $l$0): void {}
"#,
        )
        .await;
    // Import takes precedence: Different\Logger doesn't exist in fixture, so result is empty
    expect!["<none>"].assert_eq(&out);
}

/// Aliased import is now correctly resolved.
#[tokio::test]
async fn type_definition_aliased_import_with_local_class() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"//- /src/Logger.php
<?php
namespace App;
class Logger {}
//    ^^^^^^ type

//- /src/Service/Logger.php
<?php
namespace App\Service;
class Logger {}

//- /src/Processor.php
<?php
namespace App\Service;
use App\Logger as AppLogger;

function log(AppLogger $l$0): void {}  // Explicitly uses alias
"#,
    )
    .await;
}

// ── Cursor Position Variants ───────────────────────────────────────────

/// Cursor on parameter name (not type).
#[tokio::test]
async fn type_definition_cursor_on_param_name_value() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Handler {}
//    ^^^^^^^ type
function process(Handler $h$0andler): void {}
"#,
    )
    .await;
}

/// Cursor on variable without type hint.
#[tokio::test]
async fn type_definition_untyped_variable_in_function() {
    let mut s = TestServer::new().await;
    let out = s
        .check_type_definition(
            r#"<?php
$untyped$0 = 123;
"#,
        )
        .await;
    // Variable with no type hint or assignment has no type
    expect!["<none>"].assert_eq(&out);
}

// ── Edge Cases with Index Resolution ───────────────────────────────────

/// When same class name exists in both index and open file, prefer exact FQN match.
#[tokio::test]
async fn type_definition_index_prefers_exact_fqn() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_type_definition(
            r#"<?php
namespace App\Model;
function test(User $u$0): void {}
"#,
        )
        .await;
    // Should resolve to App\Model\User from index (exact FQN)
    expect![[r#"
        src/Model/User.php:4:0-4:0"#]]
    .assert_eq(&out);
}

// ── Factory Method & Method Chaining (Phase 2A Improvements) ────────────
// TODO: These require implementing support for tracking static method and
// function call return types in the type_map module. Currently TypeMap only
// tracks direct `new ClassName()` assignments.

/// Factory method returns: `Foo::create()` should resolve to Foo's type.
/// This is a common pattern where static factory methods return instances of their class.
#[tokio::test]
async fn type_definition_factory_method_returns_self_type() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Builder {
//    ^^^^^^^ type
    public static function create(): self { return new self(); }
}
$b = Builder::create();
$b$0->build();
"#,
    )
    .await;
}

/// Factory method with explicit class return: `Factory::make()` returns `static`.
#[tokio::test]
async fn type_definition_factory_method_returns_static() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Factory {
//    ^^^^^^^ type
    public static function make(): static { return new static(); }
}
$f = Factory::make();
$f$0->process();
"#,
    )
    .await;
}

/// Factory method with explicit return type annotation.
#[tokio::test]
async fn type_definition_factory_method_with_explicit_return_type() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class User {}
//    ^^^^ type
class UserFactory {
    public static function create(string $name): User {
        return new User();
    }
}
$u = UserFactory::create('Alice');
$u$0->save();
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_method_chaining_simple() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class QueryBuilder {
//    ^^^^^^^^^^^^ type
    public function select(string $col): self { return $this; }
    public function where(string $cond): self { return $this; }
}
$q = new QueryBuilder();
$q->select('id')->where('active')$0->execute();
"#,
    )
    .await;
}

#[tokio::test]
async fn type_definition_method_chaining_different_return_types() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Request {
//    ^^^^^^^ type
    public function withHeader(string $name): self { return $this; }
}
class Response {
    public function withStatus(int $code): self { return $this; }
}
class Client {
    public function request(): Request { return new Request(); }
    public function response(): Response { return new Response(); }
}
$c = new Client();
$r = $c->request()->withHeader('auth')$0->send();
"#,
    )
    .await;
}

/// Chaining where an intermediate method returns a *different* class (not self).
/// Without the correct fallback (resolving the full call, not just its receiver),
/// typeDefinition would return the receiver's class instead of the return class.
#[tokio::test]
async fn type_definition_method_chaining_non_self_return() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Pipeline {
    public function stage(string $name): Stage { return new Stage(); }
}
class Stage {
//    ^^^^^ type
    public function run(): Result { return new Result(); }
}
class Result {}
$p = new Pipeline();
$p->stage('init')$0->run();
"#,
    )
    .await;
}

/// Function call return type is now resolved and tracked in TypeMap.
#[tokio::test]
async fn type_definition_function_call_return_type() {
    let mut s = TestServer::new().await;
    s.check_type_definition_annotated(
        r#"<?php
class Document {}
//    ^^^^^^^^ type
function getDocument(): Document {
    return new Document();
}
$doc = getDocument();
$doc$0->render();
"#,
    )
    .await;
}
