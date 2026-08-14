//! OOP pattern tests: inheritance, traits, interfaces, enums, nullsafe operators.

use super::*;

#[tokio::test]
async fn references_method_via_subclass_receiver_found() {
    // Method defined on a base class must also find calls on subclass receivers.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Base {
    public function wo$0rk(): void {}
    //              ^^^^ def
}
class Child extends Base {}
$c = new Child();
$c->work();
//  ^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_trait_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
trait Timestampable {
    public function touc$0hAt(): void {}
    //              ^^^^^^^ def
}
class Post {
    use Timestampable;
}
$p = new Post();
$p->touchAt();
//  ^^^^^^^ ref
$p->touchAt();
//  ^^^^^^^ ref
"#,
    )
    .await;
}

/// Trait adaptation clauses also refer to methods. `use Auditable { record as
/// audit; }` creates an alias whose calls still use the trait method body, so a
/// complete find-usages result from the original trait method should include
/// both the adaptation entry and calls through the alias.
#[tokio::test]
#[ignore = "known gap: trait method references do not include adaptation aliases and alias call sites"]
async fn references_trait_method_includes_adaptation_alias_usages() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
trait Auditable {
    public function rec$0ord(): void {}
    //              ^^^^^^ def
}
class Post {
    use Auditable {
        record as audit;
        // ^^^^^^ ref
    }
    public function save(): void {
        $this->record();
        //     ^^^^^^ ref
        $this->audit();
        //     ^^^^^ ref
    }
}
"#,
    )
    .await;
}

#[tokio::test]
async fn references_interface_method_finds_call_sites() {
    // Cursor on the interface method declaration: must find both the
    // implementing class's method declaration and call sites.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
interface Renderable {
    public function ren$0der(): string;
    //              ^^^^^^ def
}
class Page implements Renderable {
    public function render(): string { return ''; }
    //              ^^^^^^ def
}
$page = new Page();
echo $page->render();
//          ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_enum_method() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
enum Status {
    case Active;
    public function lab$0el(): string { return 'active'; }
    //              ^^^^^ def
}
echo Status::Active->label();
//                   ^^^^^ ref
echo Status::Active->label();
//                   ^^^^^ ref
"#,
    )
    .await;
}

/// Find-references from an enum case's own declaration must find every
/// `Status::Active`-style access, not just the declaration itself. Case
/// names are conventionally PascalCase (like class names), and
/// `Status::Active` is syntactically identical to real class-constant
/// access, so both the declaration-site classification and the constant
/// walker's declaration matching need explicit `EnumMemberKind::Case`
/// handling — without it, this returns only the declaration and misses
/// every real usage.
#[tokio::test]
async fn references_enum_case_from_declaration() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
enum Status {
    case Act$0ive;
    //   ^^^^^^ def
}
function describe(Status $s): string {
    return match ($s) {
        Status::Active => 'active',
        //      ^^^^^^ ref
    };
}
echo Status::Active;
//           ^^^^^^ ref
"#,
    )
    .await;
}

/// Same coverage starting from a usage site instead of the declaration —
/// pins the already-working direction so a future regression in either
/// direction is caught.
#[tokio::test]
async fn references_enum_case_from_usage() {
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
enum Status {
    case Active;
    //   ^^^^^^ def
}
echo Status::Act$0ive;
//           ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_nullsafe_method_call() {
    // `$obj?->method()` must be found as a reference alongside `$obj->method()`.
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
class Mailer {
    public function se$0nd(): void {}
    //              ^^^^ def
}
$m = new Mailer();
$m->send();
//  ^^^^ ref
$m?->send();
//   ^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn references_two_enums_same_method_name() {
    // Two enums that both define `label()`. Refs on `Status::label` must NOT
    // pick up `Color::label` declarations and must use the span within the
    // correct enum member (not the first occurrence of the name in the file).
    let mut s = TestServer::new().await;
    s.check_references_annotated(
        r#"<?php
enum Status {
    case Active;
    public function la$0bel(): string { return 'active'; }
    //              ^^^^^ def
}
enum Color {
    case Red;
    public function label(): string { return 'red'; }
}
echo Status::Active->label();
//                   ^^^^^ ref
"#,
    )
    .await;
}
