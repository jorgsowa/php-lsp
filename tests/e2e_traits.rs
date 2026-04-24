//! E2E tests for PHP trait resolution: trait methods hoisted into a using
//! class, hover + goto-def on trait methods via `$this`, multi-trait
//! composition. CLAUDE.md highlights trait resolution as a key design detail
//! (`ClassMembers.trait_uses` populated from `ClassMemberKind::TraitUse`), so
//! these scenarios deserve dedicated wire-protocol coverage.

mod common;

use common::TestServer;

/// Hover on a `$this->traitMethod()` call inside a class that `use`s the
/// trait must surface the trait-method signature, not null.
#[tokio::test]
async fn hover_resolves_method_inherited_from_trait() {
    let src = r#"<?php
trait Greeting {
    public function sayHello(string $name): string {
        return "Hello, {$name}";
    }
}
class Greeter {
    use Greeting;
    public function run(): string {
        return $this->sayHello('world');
    }
}
"#;
    let mut server = TestServer::new().await;
    server.open("trait_hover.php", src).await;

    // Cursor on `sayHello` inside `$this->sayHello(...)` — line 9, col 22.
    let resp = server.hover("trait_hover.php", 9, 22).await;
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("sayHello"),
        "hover on trait-inherited method must return its signature, got: {contents}"
    );
}

/// Goto-definition on a trait name inside `use Greeting;` must jump to the
/// trait declaration.
#[tokio::test]
async fn goto_definition_on_trait_use_resolves_to_trait_decl() {
    let src = r#"<?php
trait Greeting {
    public function sayHello(string $name): string {
        return "Hello, {$name}";
    }
}
class Greeter {
    use Greeting;
}
"#;
    let mut server = TestServer::new().await;
    server.open("trait_def.php", src).await;

    // Line 7 = `    use Greeting;`. Cursor on `G` of Greeting (col 8).
    let resp = server.definition("trait_def.php", 7, 8).await;
    let result = &resp["result"];
    assert!(!result.is_null(), "expected a location, got null: {resp:?}");
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    let line = loc["range"]["start"]["line"].as_u64().unwrap();
    assert_eq!(
        line, 1,
        "definition must land on trait declaration (line 1), got line {line}"
    );
}

/// Known gap: goto-definition on `$this->traitMethod()` does not yet resolve
/// to the trait method. Kept as an ignored test so the feature is tracked.
#[tokio::test]
#[ignore = "goto-def on $this->traitMethod() returns null — tracked as gap"]
async fn goto_definition_jumps_to_trait_method() {
    let src = r#"<?php
trait Greeting {
    public function sayHello(string $name): string {
        return "Hello, {$name}";
    }
}
class Greeter {
    use Greeting;
    public function run(): string {
        return $this->sayHello('world');
    }
}
"#;
    let mut server = TestServer::new().await;
    server.open("trait_def2.php", src).await;

    let resp = server.definition("trait_def2.php", 9, 22).await;
    assert!(!resp["result"].is_null(), "expected a location: {resp:?}");
}

/// Multi-trait composition: a class using two traits must resolve methods
/// from both.
#[tokio::test]
async fn hover_resolves_methods_from_multiple_traits() {
    let src = r#"<?php
trait A {
    public function alpha(): int { return 1; }
}
trait B {
    public function beta(): int { return 2; }
}
class Both {
    use A;
    use B;
    public function run(): int {
        return $this->alpha() + $this->beta();
    }
}
"#;
    let mut server = TestServer::new().await;
    server.open("multi_trait.php", src).await;

    // Line 11 = `        return $this->alpha() + $this->beta();`
    let resp = server.hover("multi_trait.php", 11, 22).await;
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("alpha"),
        "hover on alpha() must mention it, got: {contents}"
    );

    let resp = server.hover("multi_trait.php", 11, 39).await;
    let contents = resp["result"]["contents"].to_string();
    assert!(
        contents.contains("beta"),
        "hover on beta() must mention it, got: {contents}"
    );
}

/// Completion after `$this->t` inside a class using a trait must include
/// the trait's public method `tick`.
#[tokio::test]
#[ignore = "$this-> completion does not yet surface trait members — tracked as gap"]
async fn completion_on_this_arrow_includes_trait_methods() {
    // Cursor after `$this->t` — the `t` prefix forces member-completion mode
    // and avoids keyword-fallback completions from parse errors.
    let src = r#"<?php
trait Counter {
    public function tick(): void {}
    public function reset(): void {}
}
class Timer {
    use Counter;
    public function run(): void {
        $this->t;
    }
}
"#;
    let mut server = TestServer::new().await;
    server.open("trait_complete.php", src).await;

    // Line 8 = `        $this->t;`. After `$this->t` = 8 + 8 = 16.
    let resp = server.completion("trait_complete.php", 8, 16).await;
    let items = resp["result"]["items"]
        .as_array()
        .or_else(|| resp["result"].as_array())
        .expect("completion items");
    let labels: Vec<&str> = items.iter().filter_map(|i| i["label"].as_str()).collect();
    assert!(
        labels.iter().any(|l| *l == "tick"),
        "expected trait method `tick` in completions, got: {labels:?}"
    );
}
