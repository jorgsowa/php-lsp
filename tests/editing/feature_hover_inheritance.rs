//! Comprehensive hover coverage.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn hover_child_receiver_resolves_parent_method_correctly() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Animal { public function speak(): string { return '...'; } }
class Dog extends Animal {}
class Parrot { public function speak(): string { return 'hello'; } }
$d = new Dog();
$d->spea$0k();
"#,
        // mir resolves to the declaring class (Animal), not the receiver (Dog).
        // This matches standard PHP IDE behaviour and is more informative.
        // Must show Animal::speak, NOT Parrot::speak.
        expect![[r#"
            ```php
            Animal::speak(): string
            ```"#]],
    )
    .await;
}

// ── Declaration-site modifiers ────────────────────────────────────────────────

#[tokio::test]
async fn hover_inheritdoc_at_tag_form() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Base {
    /** Fetches the record. */
    public function fetch(): void {}
}
class Child extends Base {
    /** @inheritDoc */
    public function fetch(): void {}
}
$c = new Child();
$c->fet$0ch();
"#,
        expect![[r#"
            ```php
            Child::fetch(): void
            ```

            ---

            Fetches the record."#]],
    )
    .await;
}

#[tokio::test]
async fn hover_inheritdoc_shows_parent_description() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Base {
    /** Sends the payload to the remote endpoint. */
    public function send(): void {}
}
class Child extends Base {
    /** {@inheritDoc} */
    public function send(): void {}
}
$c = new Child();
$c->sen$0d();
"#,
        expect![[r#"
            ```php
            Child::send(): void
            ```

            ---

            Sends the payload to the remote endpoint."#]],
    )
    .await;
}

#[tokio::test]
async fn hover_inherited_method_shows_child_class_name() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Animal { public function spe$0ak(): string { return '...'; } }
class Dog extends Animal {}
$d = new Dog();
$d->speak();
"#,
        // Hovering on the declaration itself — should show Animal::speak.
        expect![[r#"
            ```php
            public function speak(): string
            ```"#]],
    )
    .await;
}

/// `$dog->speak()` where Dog extends Animal must show Dog::speak (via extends
/// walk) when another class also has a method called `speak`.
#[tokio::test]
async fn hover_multi_trait_alpha() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
trait A { public function alpha(): int { return 1; } }
trait B { public function beta(): int { return 2; } }
class Both {
    use A;
    use B;
    public function run(): int { return $this->$0alpha() + $this->beta(); }
}
"#,
        // mir shows the declaring trait (A), not the receiver class (Both).
        expect![[r#"
            ```php
            A::alpha(): int
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_multi_trait_beta() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
trait A { public function alpha(): int { return 1; } }
trait B { public function beta(): int { return 2; } }
class Both {
    use A;
    use B;
    public function run(): int { return $this->alpha() + $this->$0beta(); }
}
"#,
        // mir shows the declaring trait (B), not the receiver class (Both).
        expect![[r#"
            ```php
            B::beta(): int
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_trait_identifier() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
trait Logg$0able { public function log(): void {} }
"#,
        expect![[r#"
            ```php
            trait Loggable
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_trait_inherited_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
trait Greeting {
    public function sayHello(string $name): string {
        return "Hello, {$name}";
    }
}
class Greeter {
    use Greeting;
    public function run(): string {
        return $this->$0sayHello('world');
    }
}
"#,
        // mir shows the declaring trait (Greeting), not the receiver (Greeter).
        expect![[r#"
            ```php
            Greeting::sayHello(string $name): string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_trait_method_picks_correct_class_not_unrelated_one() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
trait Pingable { public function ping(): string { return 'pong'; } }
class Server { use Pingable; }
class Client { public function ping(): bool { return false; } }
$s = new Server();
$s->pin$0g();
"#,
        // mir shows the declaring trait (Pingable), not the receiver (Server).
        // Correctly returns string (from Pingable), not bool (from Client::ping).
        expect![[r#"
            ```php
            Pingable::ping(): string
            ```"#]],
    )
    .await;
}
