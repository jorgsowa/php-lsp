//! Extract method code action transformation tests.
//! Tests verify that selected statements inside a method are extracted into a new private method.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn extract_method_void_no_parameters() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Logger {
    public function run(): void {
        $0echo "Starting";
        echo "Processing";$0
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Logger {
            public function run(): void {
                $this->extractedMethod();
            }

            private function extractedMethod(): void
            {
                echo "Starting";
                echo "Processing";
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_with_parameter() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Greeter {
    public function greet(): void {
        $name = "Alice";
        $0echo "Hello, " . $name;
        echo "Welcome!";$0
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Greeter {
            public function greet(): void {
                $name = "Alice";
                $this->extractedMethod($name);
            }

            private function extractedMethod(mixed $name): void
            {
                echo "Hello, " . $name;
                echo "Welcome!";
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_with_return_value() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Calculator {
    public function compute(): int {
        $a = 10;
        $0$result = $a * 2;
        $result += 5;$0
        return $result;
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Calculator {
            public function compute(): int {
                $a = 10;
                $result = $this->extractedMethod($a);
                return $result;
            }

            private function extractedMethod(mixed $a): mixed
            {
                $result = $a * 2;
                $result += 5;
                return $result;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_with_multiple_parameters() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Math {
    public function process(): void {
        $x = 5;
        $y = 3;
        $0echo "X: " . $x;
        echo "Y: " . $y;$0
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Math {
            public function process(): void {
                $x = 5;
                $y = 3;
                $this->extractedMethod($x, $y);
            }

            private function extractedMethod(mixed $x, mixed $y): void
            {
                echo "X: " . $x;
                echo "Y: " . $y;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_static_context() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Helper {
    public static function run(): void {
        $0echo "Static context";
        $something = 1;$0
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Helper {
            public static function run(): void {
                self::extractedMethod();
            }

            private static function extractedMethod(): void
            {
                echo "Static context";
                $something = 1;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_with_indentation_preserved() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Processor {
    public function handle(): void {
        if (true) {
            $0$x = 1;
            $y = 2;$0
        }
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Processor {
            public function handle(): void {
                if (true) {
                    $this->extractedMethod();
                }
            }

            private function extractedMethod(): void
            {
                $x = 1;
                $y = 2;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_with_comments_in_selection() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Logger {
    public function log(): void {
        $0// Start logging
        echo "test";
        // End logging$0
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Logger {
            public function log(): void {
                $this->extractedMethod();
            }

            private function extractedMethod(): void
            {
                // Start logging
                echo "test";
                // End logging
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_no_action_for_single_line_selection() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Foo {
    public function run(): void {
        $0$x = 1;$0
    }
}
"#,
            "Extract method",
        )
        .await;
    expect!["<action not found: Extract method>"].assert_eq(&out);
}

/// Regression test for a confirmed bug: a variable bound by the enclosing
/// method's own parameter list (never assigned via `$x = ...` inside the
/// method body) was invisible to the "vars bound before the selection"
/// scan, so it was dropped from both the extracted method's signature and
/// its call site — producing an undefined-variable error at runtime.
#[tokio::test]
async fn extract_method_uses_enclosing_method_parameter() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Greeter {
    public function greet($name): void {
        $0echo "Hello, ";
        echo $name;$0
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Greeter {
            public function greet($name): void {
                $this->extractedMethod($name);
            }

            private function extractedMethod(mixed $name): void
            {
                echo "Hello, ";
                echo $name;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_no_action_outside_class() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
$0$a = 1;
$b = 2;
$c = 3;$0
"#,
            "Extract method",
        )
        .await;
    expect!["<action not found: Extract method>"].assert_eq(&out);
}

/// Same-named variables in other methods neither become parameters nor a return value.
#[tokio::test]
async fn extract_method_ignores_variables_of_other_methods() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Job {
    public function setup(): void {
        $x = 1;
    }

    public function run(): void {
        $0$x = 2;
        echo $x;$0
    }

    public function after(): void {
        echo $x;
    }
}
"#,
            "Extract method",
        )
        .await;
    expect![[r#"
        <?php
        class Job {
            public function setup(): void {
                $x = 1;
            }

            public function run(): void {
                $this->extractedMethod();
            }

            public function after(): void {
                echo $x;
            }

            private function extractedMethod(): void
            {
                $x = 2;
                echo $x;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_method_no_action_when_selection_includes_signature() {
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
class Foo {
    $0public function run(): void {
        echo 1;$0
    }
}
"#,
            "Extract method",
        )
        .await;
    expect!["<action not found: Extract method>"].assert_eq(&out);
}
