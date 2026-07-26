//! "Extract interface from class" code action tests.
//!
//! Extracts an interface from a class's public methods and adds an `implements`
//! clause to the class declaration.

use super::*;
use expect_test::expect;

// ── Happy path ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn extract_interface_single_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class $0Service$0 {
    public function process(string $input): string { return $input; }
}
"#,
            "Extract interface 'ServiceInterface'",
        )
        .await;
    expect![[r#"
        <?php
        interface ServiceInterface
        {
            public function process(string $input): string;
        }

        class Service implements ServiceInterface {
            public function process(string $input): string { return $input; }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_interface_multiple_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class $0Repository$0 {
    public function find(int $id): ?object { return null; }
    public function save(object $entity): void {}
    public function delete(int $id): bool { return true; }
}
"#,
            "Extract interface 'RepositoryInterface'",
        )
        .await;
    expect![[r#"
        <?php
        interface RepositoryInterface
        {
            public function find(int $id): ?object;
            public function save(object $entity): void;
            public function delete(int $id): bool;
        }

        class Repository implements RepositoryInterface {
            public function find(int $id): ?object { return null; }
            public function save(object $entity): void {}
            public function delete(int $id): bool { return true; }
        }
    "#]]
    .assert_eq(&out);
}

/// A public method brought in via `use SomeTrait;` is genuinely part of the
/// class's public API and must appear in the extracted interface, not just
/// methods declared directly in the class body.
#[tokio::test]
async fn extract_interface_includes_trait_provided_public_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
trait Greets {
    public function greet(): string { return 'hi'; }
}
class $0Person$0 {
    use Greets;
    public function name(): string { return 'Bob'; }
}
"#,
            "Extract interface 'PersonInterface'",
        )
        .await;
    expect![[r#"
        <?php
        trait Greets {
            public function greet(): string { return 'hi'; }
        }
        interface PersonInterface
        {
            public function name(): string;
            public function greet(): string;
        }

        class Person implements PersonInterface {
            use Greets;
            public function name(): string { return 'Bob'; }
        }
    "#]]
    .assert_eq(&out);
}

/// A class's own override of a trait method wins — the trait's signature
/// must not also appear, which would produce a duplicate interface member.
#[tokio::test]
async fn extract_interface_class_override_of_trait_method_not_duplicated() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
trait Greets {
    public function greet(): string { return 'hi'; }
}
class $0Person$0 {
    use Greets;
    public function greet(): string { return 'hello'; }
}
"#,
            "Extract interface 'PersonInterface'",
        )
        .await;
    expect![[r#"
        <?php
        trait Greets {
            public function greet(): string { return 'hi'; }
        }
        interface PersonInterface
        {
            public function greet(): string;
        }

        class Person implements PersonInterface {
            use Greets;
            public function greet(): string { return 'hello'; }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_interface_excludes_non_public_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class $0Handler$0 {
    public function handle(string $cmd): void {}
    protected function prepare(string $cmd): string { return $cmd; }
    private function log(string $msg): void {}
}
"#,
            "Extract interface 'HandlerInterface'",
        )
        .await;
    expect![[r#"
        <?php
        interface HandlerInterface
        {
            public function handle(string $cmd): void;
        }

        class Handler implements HandlerInterface {
            public function handle(string $cmd): void {}
            protected function prepare(string $cmd): string { return $cmd; }
            private function log(string $msg): void {}
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_interface_excludes_constructor_and_destructor() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class $0Worker$0 {
    public function __construct(private string $name) {}
    public function __destruct() {}
    public function run(): void {}
}
"#,
            "Extract interface 'WorkerInterface'",
        )
        .await;
    expect![[r#"
        <?php
        interface WorkerInterface
        {
            public function run(): void;
        }

        class Worker implements WorkerInterface {
            public function __construct(private string $name) {}
            public function __destruct() {}
            public function run(): void {}
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_interface_with_no_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class $0Notifier$0 {
    public function send($message) {}
}
"#,
            "Extract interface 'NotifierInterface'",
        )
        .await;
    expect![[r#"
        <?php
        interface NotifierInterface
        {
            public function send($message);
        }

        class Notifier implements NotifierInterface {
            public function send($message) {}
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_interface_with_static_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class $0Factory$0 {
    public static function create(string $type): self { return new self(); }
}
"#,
            "Extract interface 'FactoryInterface'",
        )
        .await;
    expect![[r#"
        <?php
        interface FactoryInterface
        {
            public static function create(string $type): self;
        }

        class Factory implements FactoryInterface {
            public static function create(string $type): self { return new self(); }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_interface_appends_to_existing_implements() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Stringable { public function __toString(): string; }
class $0Model$0 implements Stringable {
    public function __toString(): string { return ''; }
    public function getId(): int { return 0; }
}
"#,
            "Extract interface 'ModelInterface'",
        )
        .await;
    expect![[r#"
        <?php
        interface Stringable { public function __toString(): string; }
        interface ModelInterface
        {
            public function __toString(): string;
            public function getId(): int;
        }

        class Model implements Stringable, ModelInterface {
            public function __toString(): string { return ''; }
            public function getId(): int { return 0; }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_interface_with_union_return_type() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class $0Converter$0 {
    public function convert(mixed $value): int|string|null { return null; }
}
"#,
            "Extract interface 'ConverterInterface'",
        )
        .await;
    expect![[r#"
        <?php
        interface ConverterInterface
        {
            public function convert(mixed $value): int|string|null;
        }

        class Converter implements ConverterInterface {
            public function convert(mixed $value): int|string|null { return null; }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_interface_with_unbraced_namespace() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
namespace App\Services;

class $0Logger$0 {
    public function log(string $message): void {}
}
"#,
            "Extract interface 'LoggerInterface'",
        )
        .await;
    expect![[r#"
        <?php
        namespace App\Services;

        interface LoggerInterface
        {
            public function log(string $message): void;
        }

        class Logger implements LoggerInterface {
            public function log(string $message): void {}
        }
    "#]]
    .assert_eq(&out);
}

// ── Cursor position ───────────────────────────────────────────────────────────

#[tokio::test]
async fn extract_interface_offered_anywhere_on_class_declaration() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    // Cursor on the `class` keyword itself.
    let out = s
        .check_code_actions(
            r#"<?php
$0class$0 Foo {
    public function bar(): void {}
}
"#,
        )
        .await;
    assert!(
        out.contains("Extract interface 'FooInterface'"),
        "expected action in: {out}"
    );
}

#[tokio::test]
async fn extract_interface_not_offered_when_cursor_inside_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Foo {
    public function $0bar$0(): void {}
}
"#,
        )
        .await;
    assert!(
        !out.contains("Extract interface"),
        "action must not appear inside method body: {out}"
    );
}

// ── Not offered ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn extract_interface_not_offered_when_no_eligible_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class $0Bag$0 {
    private string $data = '';
    protected function init(): void {}
}
"#,
        )
        .await;
    assert!(
        !out.contains("Extract interface"),
        "action must not appear when no public methods: {out}"
    );
}

#[tokio::test]
async fn extract_interface_not_offered_when_already_implements_interface() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class $0Foo$0 implements FooInterface {
    public function bar(): void {}
}
"#,
        )
        .await;
    assert!(
        !out.contains("Extract interface 'FooInterface'"),
        "action must not be offered when class already implements FooInterface: {out}"
    );
}

// ── Action title ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn extract_interface_action_title_uses_class_name() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class $0EmailSender$0 {
    public function send(string $to, string $body): bool { return true; }
}
"#,
        )
        .await;
    assert!(
        out.contains("Extract interface 'EmailSenderInterface'"),
        "expected action in: {out}"
    );
}
