//! Change visibility code action: "Make public / protected / private".

use super::*;
use expect_test::expect;

// --- Availability ---

#[tokio::test]
async fn change_visibility_offers_two_alternatives_for_public_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Greeter {
    $0public function hello(string $name): string { return $name; }
}
"#,
        )
        .await;
    // Both "Make protected" and "Make private" must appear.
    assert!(
        out.contains("Make protected"),
        "expected 'Make protected' in actions, got: {out}"
    );
    assert!(
        out.contains("Make private"),
        "expected 'Make private' in actions, got: {out}"
    );
    assert!(
        !out.contains("Make public"),
        "should not offer 'Make public' when already public, got: {out}"
    );
}

#[tokio::test]
async fn change_visibility_offers_two_alternatives_for_protected_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Base {
    $0protected function helper(): void {}
}
"#,
        )
        .await;
    assert!(
        out.contains("Make public"),
        "expected 'Make public' in actions, got: {out}"
    );
    assert!(
        out.contains("Make private"),
        "expected 'Make private' in actions, got: {out}"
    );
    assert!(
        !out.contains("Make protected"),
        "should not offer 'Make protected' when already protected, got: {out}"
    );
}

#[tokio::test]
async fn change_visibility_offers_two_alternatives_for_private_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class User {
    $0private string $name = '';
}
"#,
        )
        .await;
    assert!(
        out.contains("Make public"),
        "expected 'Make public' in actions, got: {out}"
    );
    assert!(
        out.contains("Make protected"),
        "expected 'Make protected' in actions, got: {out}"
    );
    assert!(
        !out.contains("Make private"),
        "should not offer 'Make private' when already private, got: {out}"
    );
}

/// An attribute argument string containing a visibility keyword (e.g. an
/// `Assert\Choice(choices: ["private", "public"])` list) must not be matched
/// instead of the real modifier — `find_visibility_range` used to search from
/// the member's span start, which includes leading attributes.
#[tokio::test]
async fn change_visibility_ignores_keyword_inside_attribute_string() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class User {
    #[Assert\Choice(choices: ["private", "public"])]
    $0private string $mode = '';
}
"#,
            "Make public",
        )
        .await;
    expect![[r#"
        <?php
        class User {
            #[Assert\Choice(choices: ["private", "public"])]
            public string $mode = '';
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn change_visibility_no_action_inside_method_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Foo {
    public function bar(): void {
        $x$0 = 1;
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Make public"),
        "should not offer visibility change inside method body, got: {out}"
    );
    assert!(
        !out.contains("Make protected"),
        "should not offer visibility change inside method body, got: {out}"
    );
    assert!(
        !out.contains("Make private"),
        "should not offer visibility change inside method body, got: {out}"
    );
}

// --- Applied edits ---

#[tokio::test]
async fn change_visibility_public_to_protected_applies_edit() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Greeter {
    $0public function hello(string $name): string { return $name; }
}
"#,
            "Make protected",
        )
        .await;
    expect![[r#"
        <?php
        class Greeter {
            protected function hello(string $name): string { return $name; }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn change_visibility_public_to_private_applies_edit() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Config {
    $0public static function defaults(): array { return []; }
}
"#,
            "Make private",
        )
        .await;
    expect![[r#"
        <?php
        class Config {
            private static function defaults(): array { return []; }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn change_visibility_protected_to_public_applies_edit() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Service {
    $0protected string $endpoint = 'https://api.example.com';
}
"#,
            "Make public",
        )
        .await;
    expect![[r#"
        <?php
        class Service {
            public string $endpoint = 'https://api.example.com';
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn change_visibility_private_const_to_public() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Config {
    $0private const VERSION = '1.0.0';
}
"#,
            "Make public",
        )
        .await;
    expect![[r#"
        <?php
        class Config {
            public const VERSION = '1.0.0';
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn change_visibility_trait_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
trait Logger {
    $0private function log(string $msg): void {}
}
"#,
            "Make protected",
        )
        .await;
    expect![[r#"
        <?php
        trait Logger {
            protected function log(string $msg): void {}
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn change_visibility_abstract_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
abstract class Shape {
    $0protected abstract function area(): float;
}
"#,
            "Make public",
        )
        .await;
    expect![[r#"
        <?php
        abstract class Shape {
            public abstract function area(): float;
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn change_visibility_final_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Base {
    $0public final function lock(): void {}
}
"#,
            "Make protected",
        )
        .await;
    expect![[r#"
        <?php
        class Base {
            protected final function lock(): void {}
        }
    "#]]
    .assert_eq(&out);
}
