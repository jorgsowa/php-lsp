//! Comprehensive attribute references across all declaration types.
//!
//! Tests attribute usage on:
//! - Classes, abstract classes, final classes
//! - Methods, static methods
//! - Properties, static properties
//! - Functions
//! - Parameters (function, method, property promotion)
//! - Enums and enum cases
//! - Multiple attributes on same element

use super::*;

#[tokio::test]
async fn attribute_on_class() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Entity$0Attribute {}
//    ^^^^^^^^^^^^^^^ def

#[EntityAttribute]
//^^^^^^^^^^^^^^^ ref
class User {}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Route$0Attribute {}
//    ^^^^^^^^^^^^^^ def

class Controller {
    #[RouteAttribute('/users', 'GET')]
    //^^^^^^^^^^^^^^ ref
    public function getUsers(): array {
        return [];
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_property() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Serialize$0Attribute {}
//    ^^^^^^^^^^^^^^^^^^ def

class Document {
    #[SerializeAttribute]
    //^^^^^^^^^^^^^^^^^^ ref
    public string $content;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_function() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Depreca$0ted {}
//    ^^^^^^^^^^ def

#[Deprecated]
//^^^^^^^^^^ ref
function oldFunction(): void {
    echo 'This is deprecated';
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_parameter() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Validate$0Attribute {}
//    ^^^^^^^^^^^^^^^^^ def

function process(
    #[ValidateAttribute('email')]
    //^^^^^^^^^^^^^^^^^ ref
    string $email
): void {
    // validate
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_promoted_property() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Immuta$0ble {}
//    ^^^^^^^^^ def

class User {
    public function __construct(
        #[Immutable]
        //^^^^^^^^^ ref
        public readonly string $id,
    ) {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_enum() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Backable$0Enum {}
//    ^^^^^^^^^^^^ def

#[BackableEnum]
//^^^^^^^^^^^^ ref
enum Status: int {
    case Active = 1;
    case Inactive = 0;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_enum_case() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Default$0Case {}
//    ^^^^^^^^^^^ def

enum Priority {
    #[DefaultCase]
    //^^^^^^^^^^^ ref
    case Low;
    case Medium;
    case High;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_multiple_on_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Async {}
class Cach$0ed {}
//    ^^^^^^ def

class Service {
    #[Async]
    #[Cached]
    //^^^^^^ ref
    public function fetchData(): array {
        return [];
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_with_constructor_arguments() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Required$0Arg {}
//    ^^^^^^^^^^^ def

class Form {
    #[RequiredArg('email', 'string')]
    //^^^^^^^^^^^ ref
    public string $email;
}
"#,
    )
    .await;
}

/// A `new X(...)` call site living inside a PHP 8 attribute's argument list
/// (e.g. PHPUnit's `#[TestWith([new Spread(...)])]`) must be found by
/// find-references on `X`, exactly like an identical `new X(...)` in regular
/// code. This was a confirmed gap against a real ~15K-file codebase
/// (app-server verification, 2026-07-21): the plain-code instantiation was
/// found, the attribute-nested one silently was not. Regression pin — this
/// currently passes.
#[tokio::test]
async fn attribute_argument_new_expression_is_a_reference() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Spread$0 {
//    ^^^^^^ def

    public function __construct(public string $id) {}
}

class IntermediateJson {
    public function __construct(public array $spreads) {}
}

function plain(): IntermediateJson {
    return new IntermediateJson([new Spread('s0')]);
//                                   ^^^^^^ ref
}

#[TestWith([new Spread('s1')])]
//              ^^^^^^ ref
function attributeArg(): void {}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_repeatable() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Permiss$0ion {}
//    ^^^^^^^^^^ def

class AdminResource {
    #[Permission('read')]
    //^^^^^^^^^^ ref
    #[Permission('admin')]
    //^^^^^^^^^^ ref
    public function manage(): void {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_static_property() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Singleton$0Marker {}
//    ^^^^^^^^^^^^^^^ def

class Cache {
    #[SingletonMarker]
    //^^^^^^^^^^^^^^^ ref
    public static Cache $instance;

    public static function getInstance(): self {
        return self::$instance;
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_cross_file() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"//- /src/Attributes.php
<?php
class Api$0Endpoint {}
//    ^^^^^^^^^^^ def

//- /src/Controllers/UserController.php
<?php
class UserController {
    #[ApiEndpoint('/users', 'GET')]
    //^^^^^^^^^^^ ref
    public function list(): array {}

    #[ApiEndpoint('/users/{id}', 'GET')]
    //^^^^^^^^^^^ ref
    public function show(int $id): array {}
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_abstract_class() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Abstract$0Base {}
//    ^^^^^^^^^^^^ def

#[AbstractBase]
//^^^^^^^^^^^^ ref
abstract class Handler {
    abstract public function handle(): void;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_final_class() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Immutab$0le {}
//    ^^^^^^^^^ def

#[Immutable]
//^^^^^^^^^ ref
final class Config {
    public readonly string $path;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_interface_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Required$0Impl {}
//    ^^^^^^^^^^^^ def

interface Repository {
    #[RequiredImpl]
    //^^^^^^^^^^^^ ref
    public function find(int $id): mixed;
}
"#,
    )
    .await;
}

#[tokio::test]
async fn attribute_on_static_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Singleton$0Factory {}
//    ^^^^^^^^^^^^^^^^ def

class Database {
    #[SingletonFactory]
    //^^^^^^^^^^^^^^^^ ref
    public static function getInstance(): self {
        static $instance = null;
        if ($instance === null) {
            $instance = new self();
        }
        return $instance;
    }
}
"#,
    )
    .await;
}
