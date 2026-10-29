//! Type definition (`textDocument/typeDefinition`) coverage.

use super::*;

use expect_test::expect;

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

// ── Tests for indexed type definition (background files) ────

/// Type definition resolves types from background-indexed (not-opened) files.
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
    expect!["src/Model/User.php:4:6-4:10"].assert_eq(&out);
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
    expect!["src/Model/User.php:4:6-4:10"].assert_eq(&out);
}

/// Generic-like syntax (`Collection<User>`) is not resolved; type definition returns nothing.
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
    expect!["<none>"].assert_eq(&out);
}

// ── $var FQN resolution ───────────────────────────────────────────────────────

// ── index (background file) FQN resolution ───────────────────────────────────

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
    expect!["src/Model/User.php:4:6-4:10"].assert_eq(&out);
}

/// Type definition for a `use`-aliased type resolves from a background-indexed file.
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
    expect!["src/Model/User.php:4:6-4:10"].assert_eq(&out);
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

// ── Self, Parent, Static Keywords ────────────────────────────────────────

// ── Trait-Specific Cases ───────────────────────────────────────────────

// ── Enum Backed Types ─────────────────────────────────────────────────

// ── Interface Inheritance ──────────────────────────────────────────────

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

// ── Cursor Position Variants ───────────────────────────────────────────

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

/// A same-named `User` class is open locally (in `namespace Other`) and a
/// different `User` lives in the index (`App\Model\User`). An FQN-qualified
/// reference to `App\Model\User` must resolve to the indexed class, not the
/// unrelated open-doc class that only shares the short name.
#[tokio::test]
async fn type_definition_index_prefers_exact_fqn() {
    let mut s = TestServer::with_fixture("psr4-mini").await;
    s.wait_for_index_ready().await;

    let out = s
        .check_type_definition(
            r#"//- /other.php
<?php
namespace Other;
class User {}

//- /main.php
<?php
namespace App\Model;
function test(\App\Model\User $u$0): void {}
"#,
        )
        .await;
    // Should resolve to App\Model\User from the index, not Other\User.
    expect!["src/Model/User.php:4:6-4:10"].assert_eq(&out);
}

// ── Factory Method & Method Chaining ────────────────────────────────────
