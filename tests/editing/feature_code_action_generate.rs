//! Generate code action transformation tests.
//! Tests verify that constructors and getters/setters are generated from properties.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn generate_constructor_from_properties() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class U$0ser$0 {
    public string $name = '';
    public int $age = 0;
}
"#,
            "Generate constructor",
        )
        .await;
    expect![[r#"
        <?php
        class User {
            public string $name = '';
            public int $age = 0;
            public function __construct(
                string $name,
                int $age,
            ) {
                $this->name = $name;
                $this->age = $age;
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn generate_constructor_with_nullable_types() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class P$0roduct$0 {
    public string $title;
    public ?string $description = null;
}
"#,
            "Generate constructor",
        )
        .await;
    expect![[r#"
        <?php
        class Product {
            public string $title;
            public ?string $description = null;
            public function __construct(
                string $title,
                ?string $description,
            ) {
                $this->title = $title;
                $this->description = $description;
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn generate_getters_and_setters() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class A$0ccount$0 {
    private string $email = '';
    private int $balance = 0;
}
"#,
            "Generate 2 getters/setters",
        )
        .await;
    expect![[r#"
        <?php
        class Account {
            private string $email = '';
            private int $balance = 0;
            public function getEmail(): string
            {
                return $this->email;
            }

            public function setEmail(string $email): void
            {
                $this->email = $email;
            }

            public function getBalance(): int
            {
                return $this->balance;
            }

            public function setBalance(int $balance): void
            {
                $this->balance = $balance;
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn generate_getters_single_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class C$0onfig$0 {
    private string $apiKey = '';
}
"#,
            "Generate getter/setter",
        )
        .await;
    expect![[r#"
        <?php
        class Config {
            private string $apiKey = '';
            public function getApiKey(): string
            {
                return $this->apiKey;
            }

            public function setApiKey(string $apiKey): void
            {
                $this->apiKey = $apiKey;
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn generate_no_action_when_constructor_exists() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class U$0ser$0 {
    public string $name = '';
    public function __construct(string $name) { $this->name = $name; }
}
"#,
            "Generate constructor",
        )
        .await;
    expect!["<action not found: Generate constructor>"].assert_eq(&out);
}

#[tokio::test]
async fn generate_constructor_no_properties() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class E$0mpty$0 {
}
"#,
            "Generate constructor",
        )
        .await;
    expect!["<action not found: Generate constructor>"].assert_eq(&out);
}

/// Generated constructor params currently drop each property's own default
/// value — `collect_constructor`'s `Prop` only tracks name/type, not the
/// initializer. Pinning today's (defaults-dropping) behavior; if default
/// propagation is ever added, this snapshot should gain `= 'John'`/`= 0`.
#[tokio::test]
async fn generate_constructor_does_not_propagate_property_defaults() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class U$0ser$0 {
    public string $name = 'John';
    public int $age = 0;
}
"#,
            "Generate constructor",
        )
        .await;
    expect![[r#"
        <?php
        class User {
            public string $name = 'John';
            public int $age = 0;
            public function __construct(
                string $name,
                int $age,
            ) {
                $this->name = $name;
                $this->age = $age;
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn generate_getters_with_boolean_properties() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class S$0ettings$0 {
    private bool $isEnabled = false;
}
"#,
            "Generate getter/setter",
        )
        .await;
    expect![[r#"
        <?php
        class Settings {
            private bool $isEnabled = false;
            public function getIsEnabled(): bool
            {
                return $this->isEnabled;
            }

            public function setIsEnabled(bool $isEnabled): void
            {
                $this->isEnabled = $isEnabled;
            }

        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn generate_skips_existing_getter_generates_missing_setter() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class U$0ser$0 {
    private string $name;
    public function getName(): string { return $this->name; }
}
"#,
            "Generate setter",
        )
        .await;
    expect![[r#"
        <?php
        class User {
            private string $name;
            public function getName(): string { return $this->name; }
            public function setName(string $name): void
            {
                $this->name = $name;
            }

        }
    "#]]
    .assert_eq(&out);
}

/// A `readonly` property can only be assigned once, from the constructor —
/// a generated public setter would be a PHP fatal error at the second call.
/// Only a getter must be generated.
#[tokio::test]
async fn generate_getter_only_for_readonly_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class P$0oint$0 {
    public readonly float $x;
}
"#,
            "Generate getter",
        )
        .await;
    expect![[r#"
        <?php
        class Point {
            public readonly float $x;
            public function getX(): float
            {
                return $this->x;
            }

        }
    "#]]
    .assert_eq(&out);
}

/// Same as above but for a promoted constructor property declared
/// `public readonly` directly on the `__construct` parameter.
#[tokio::test]
async fn generate_getter_only_for_readonly_promoted_property() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class P$0oint$0 {
    public function __construct(public readonly float $x) {}
}
"#,
            "Generate getter",
        )
        .await;
    expect![[r#"
        <?php
        class Point {
            public function __construct(public readonly float $x) {}
            public function getX(): float
            {
                return $this->x;
            }

        }
    "#]]
    .assert_eq(&out);
}

/// A `readonly class` (PHP 8.2+) makes every property readonly even
/// without a per-property `readonly` keyword.
#[tokio::test]
async fn generate_getter_only_for_property_in_readonly_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
readonly class P$0oint$0 {
    public float $x;
}
"#,
            "Generate getter",
        )
        .await;
    expect![[r#"
        <?php
        readonly class Point {
            public float $x;
            public function getX(): float
            {
                return $this->x;
            }

        }
    "#]]
    .assert_eq(&out);
}
