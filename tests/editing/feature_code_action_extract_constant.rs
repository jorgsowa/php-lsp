//! Extract constant code action transformation tests.
//! Tests verify that selected literals are extracted into named constants.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn extract_constant_string_in_class() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Greeter {
    public function greet(): string {
        return $0"Hello, World!"$0;
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        class Greeter {
            private const HELLO_WORLD = "Hello, World!";
            public function greet(): string {
                return self::HELLO_WORLD;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_integer_in_class() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Timer {
    public function delay(): void {
        sleep($042$0);
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        class Timer {
            private const CONSTANT_42 = 42;
            public function delay(): void {
                sleep(self::CONSTANT_42);
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_float_in_class() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Calculator {
    public function ratio(): float {
        return $03.14$0;
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        class Calculator {
            private const CONSTANT_3_14 = 3.14;
            public function ratio(): float {
                return self::CONSTANT_3_14;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_at_file_scope() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
function getName() {
    return $0"app"$0;
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        const APP = "app";
        function getName() {
            return APP;
        }
    "#]]
    .assert_eq(&out);
}

/// Selecting the RHS of an existing `const NAME = value;` must not offer the
/// action — extracting it would insert a second `const ACTIVE = ...;` above,
/// redeclaring the same constant name (a PHP fatal error).
#[tokio::test]
async fn extract_constant_already_in_const_declaration_returns_no_action() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
interface Status {
    const ACTIVE = $0"active"$0;
}
"#,
            "Extract constant",
        )
        .await;
    expect!["<action not found: Extract constant>"].assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_in_trait() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
trait Logging {
    public function log(): void {
        $level = $0"info"$0;
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        trait Logging {
            private const INFO = "info";
            public function log(): void {
                $level = self::INFO;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_after_interface_at_file_scope() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
interface PaymentGateway {
    public function charge(): void;
}
$fee = $0250$0;
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        const CONSTANT_250 = 250;
        interface PaymentGateway {
            public function charge(): void;
        }
        $fee = CONSTANT_250;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_non_literal_returns_no_action() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = $0foo()$0;
"#,
            "Extract constant",
        )
        .await;
    expect!["<action not found: Extract constant>"].assert_eq(&out);
}

/// A class/interface constant initializer must be a compile-time constant
/// expression. A double-quoted string with simple `$var` interpolation is
/// not one — hoisting it verbatim would be a PHP fatal error ("Constant
/// expression contains invalid operations").
#[tokio::test]
async fn extract_constant_interpolated_string_returns_no_action() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$name = "world";
$x = $0"Hello $name"$0;
"#,
            "Extract constant",
        )
        .await;
    expect!["<action not found: Extract constant>"].assert_eq(&out);
}

/// Same as above but for complex `{$expr}` interpolation syntax.
#[tokio::test]
async fn extract_constant_complex_interpolated_string_returns_no_action() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Foo {
    public $x = 1;
    public function bar() {
        $y = $0"Value: {$this->x}"$0;
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect!["<action not found: Extract constant>"].assert_eq(&out);
}

/// A `$` that can't start a variable name (not followed by a letter/`_`/`{`)
/// does not interpolate in PHP, so it must still be extractable.
#[tokio::test]
async fn extract_constant_dollar_sign_without_interpolation_is_allowed() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = $0"price: $5"$0;
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        const PRICE_5 = "price: $5";
        $x = PRICE_5;
    "#]]
    .assert_eq(&out);
}

/// A single-quoted string never interpolates in PHP regardless of content,
/// so a literal `$name` inside one must still be extractable.
#[tokio::test]
async fn extract_constant_single_quoted_dollar_is_allowed() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = $0'Hello $name'$0;
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        const HELLO_NAME = 'Hello $name';
        $x = HELLO_NAME;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_file_scope_inserts_before_use_statement() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = $0"hello"$0;
use Foo\Bar;
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        const HELLO = "hello";
        $x = HELLO;
        use Foo\Bar;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_url_string_derives_screaming_snake_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class ApiClient {
    public function endpoint(): string {
        return $0"https://api.example.com"$0;
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        class ApiClient {
            private const HTTPS_API_EXAMPLE_COM = "https://api.example.com";
            public function endpoint(): string {
                return self::HTTPS_API_EXAMPLE_COM;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_all_special_chars_falls_back_to_default_name() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Symbols {
    public function noise(): string {
        return $0"!!!"$0;
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        class Symbols {
            private const EXTRACTED_CONSTANT = "!!!";
            public function noise(): string {
                return self::EXTRACTED_CONSTANT;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_class_scope_when_method_body_has_brace_in_string() {
    // Regression: `find_matching_close` used to count `{` / `}` inside string
    // literals as structural braces, so `"hello { world }"` caused it to return
    // the wrong closing line and fall back to file scope instead of class scope.
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Formatter {
    public function wrap(): string {
        $template = "{ content }";
        return $0"output"$0;
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        class Formatter {
            private const OUTPUT = "output";
            public function wrap(): string {
                $template = "{ content }";
                return self::OUTPUT;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_constant_class_scope_when_method_has_line_comment_with_brace() {
    // Regression: `}` inside a `//` comment must not decrement the brace depth.
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Builder {
    public function build(): string {
        // end of block: }
        return $0"result"$0;
    }
}
"#,
            "Extract constant",
        )
        .await;
    expect![[r#"
        <?php
        class Builder {
            private const RESULT = "result";
            public function build(): string {
                // end of block: }
                return self::RESULT;
            }
        }
    "#]]
    .assert_eq(&out);
}
