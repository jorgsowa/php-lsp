//! Code action: "Convert '$var' to instance property"
//!
//! Converts a local variable in a non-static method body to a class property:
//! inserts `private $prop;` before the first class member and replaces every
//! `$var` occurrence inside the method body with `$this->prop`.

use super::*;
use expect_test::expect;

// ── Availability ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn local_to_property_offered_for_local_variable() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Counter {
    public function increment(): void {
        $coun$0t = 0;
        $count++;
    }
}
"#,
        )
        .await;
    assert!(
        out.contains("Convert '$count' to instance property"),
        "expected action in: {out}"
    );
}

#[tokio::test]
async fn local_to_property_not_offered_for_this() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Foo {
    public function bar(): void {
        $thi$0s->x = 1;
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert '$this' to instance property"),
        "should not offer for $this, got: {out}"
    );
}

#[tokio::test]
async fn local_to_property_not_offered_for_parameter() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Greeter {
    public function greet(string $nam$0e): string {
        return "Hello, $name!";
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert '$name' to instance property"),
        "should not offer for a parameter, got: {out}"
    );
}

#[tokio::test]
async fn local_to_property_not_offered_for_static_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Factory {
    public static function create(): self {
        $instanc$0e = new self();
        return $instance;
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert '$instance' to instance property"),
        "should not offer inside a static method, got: {out}"
    );
}

/// A blind text-scan replacement inside a nested closure's `use ($var)`
/// capture clause would produce `use ($this->prop)` — a syntax error. The
/// action must not be offered at all when the variable is referenced inside
/// a nested closure.
#[tokio::test]
async fn local_to_property_not_offered_when_used_inside_nested_closure() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Counter {
    public function bar(): void {
        $coun$0t = 0;
        $add = function () use ($count) { return $count + 1; };
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert '$count' to instance property"),
        "should not offer when var is captured by a nested closure, got: {out}"
    );
}

/// Same hazard via an arrow function, which implicitly captures outer
/// variables by value without a `use` clause — still not safe to rewrite.
#[tokio::test]
async fn local_to_property_not_offered_when_used_inside_arrow_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Counter {
    public function bar(): void {
        $coun$0t = 0;
        $add = fn($x) => $x + $count;
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert '$count' to instance property"),
        "should not offer when var is used inside a nested arrow function, got: {out}"
    );
}

#[tokio::test]
async fn local_to_property_not_offered_when_property_already_exists() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_actions(
            r#"<?php
class Counter {
    private int $count = 0;
    public function reset(): void {
        $coun$0t = 0;
    }
}
"#,
        )
        .await;
    assert!(
        !out.contains("Convert '$count' to instance property"),
        "should not offer when property already exists, got: {out}"
    );
}

// ── Applied edits ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn local_to_property_basic_edit() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Counter {
    public function increment(): void {
        $coun$0t = 0;
        $count++;
    }
}
"#,
            "Convert '$count' to instance property",
        )
        .await;
    expect![[r#"
        <?php
        class Counter {
            private $count;
            public function increment(): void {
                $this->count = 0;
                $this->count++;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn local_to_property_replaces_all_occurrences() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Report {
    public function generate(): string {
        $titl$0e = 'Annual';
        $title .= ' Report';
        return $title;
    }
}
"#,
            "Convert '$title' to instance property",
        )
        .await;
    expect![[r#"
        <?php
        class Report {
            private $title;
            public function generate(): string {
                $this->title = 'Annual';
                $this->title .= ' Report';
                return $this->title;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn local_to_property_inserted_before_existing_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Service {
    public function __construct(private string $name) {}

    public function run(): void {
        $resul$0t = $this->compute();
        echo $result;
    }

    private function compute(): string {
        return $this->name;
    }
}
"#,
            "Convert '$result' to instance property",
        )
        .await;
    expect![[r#"
        <?php
        class Service {
            private $result;
            public function __construct(private string $name) {}

            public function run(): void {
                $this->result = $this->compute();
                echo $this->result;
            }

            private function compute(): string {
                return $this->name;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn local_to_property_with_existing_property_in_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Builder {
    private string $name = '';

    public function build(): array {
        $item$0s = [];
        $items[] = $this->name;
        return $items;
    }
}
"#,
            "Convert '$items' to instance property",
        )
        .await;
    expect![[r#"
        <?php
        class Builder {
            private $items;
            private string $name = '';

            public function build(): array {
                $this->items = [];
                $this->items[] = $this->name;
                return $this->items;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn local_to_property_in_namespaced_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
namespace App\Service;

class Processor {
    public function process(): void {
        $dat$0a = [];
        $data[] = 1;
    }
}
"#,
            "Convert '$data' to instance property",
        )
        .await;
    expect![[r#"
        <?php
        namespace App\Service;

        class Processor {
            private $data;
            public function process(): void {
                $this->data = [];
                $this->data[] = 1;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn local_to_property_does_not_match_similar_names() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Parser {
    public function parse(): void {
        $va$0l = 1;
        $value = 2;
        echo $val + $value;
    }
}
"#,
            "Convert '$val' to instance property",
        )
        .await;
    // Only $val should be replaced, not $value.
    expect![[r#"
        <?php
        class Parser {
            private $val;
            public function parse(): void {
                $this->val = 1;
                $value = 2;
                echo $this->val + $value;
            }
        }
    "#]]
    .assert_eq(&out);
}
