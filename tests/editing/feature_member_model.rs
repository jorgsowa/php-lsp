use super::*;
use expect_test::expect;

/// Member lines (methods/properties/constants) of a completion rendering.
fn members(out: &str) -> String {
    out.lines()
        .filter(|l| {
            l.starts_with("Method ") || l.starts_with("Property ") || l.starts_with("Constant ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn interface_receiver_completes_its_own_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
interface Shape { public function area(): float; }
class Other { public function unrelated(): void {} }
function f(Shape $x) { $x->$0 }
"#,
        )
        .await;
    expect!["Method      area"].assert_eq(&members(&out));
}

#[tokio::test]
async fn interface_receiver_includes_parent_interface_methods() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
namespace App;
interface Named { public function name(): string; }
interface Shape extends Named { public function area(): float; }
function f(Shape $x) { $x->$0 }
"#,
        )
        .await;
    expect![[r#"
        Method      area
        Method      name"#]]
    .assert_eq(&members(&out));
}

#[tokio::test]
async fn inherited_private_members_are_not_offered() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Base {
    private $hidden;
    protected $guarded;
    private function secret(): void {}
    protected function prot(): void {}
    public function pub(): void {}
}
class Child extends Base {}
$c = new Child();
$c->$0
"#,
        )
        .await;
    expect!["Method      pub"].assert_eq(&members(&out));
}

#[tokio::test]
async fn inherited_protected_members_are_offered_inside_a_subclass() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Base {
    private function secret(): void {}
    protected function prot(): void {}
    public function pub(): void {}
}
class Child extends Base {
    public function run(): void { $this->$0 }
}
"#,
        )
        .await;
    expect![[r#"
        Method      prot
        Method      pub
        Method      run"#]]
    .assert_eq(&members(&out));
}

#[tokio::test]
async fn parent_resolves_against_the_declaring_files_imports() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /a/Base.php
<?php
namespace A;
class Base { public function fromA(): void {} }

//- /b/Base.php
<?php
namespace B;
class Base { public function fromB(): void {} }

//- /a/Mid.php
<?php
namespace A;
class Mid extends Base {}

//- /main.php
<?php
use B\Base;
use A\Mid;
function f(Mid $m) { $m->$0 }
"#,
        )
        .await;
    expect!["Method      fromA"].assert_eq(&members(&out));
}

#[tokio::test]
async fn namespaced_enum_gets_match_arm_completion() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
namespace App;
enum Suit { case Hearts; case Spades; }
function f(Suit $s) {
    match ($s) {
        $0
    }
}
"#,
        )
        .await;
    let arms: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("Constant    Suit::"))
        .collect();
    expect![[r#"
        Constant    Suit::Hearts
        Constant    Suit::Spades"#]]
    .assert_eq(&arms.join("\n"));
}

#[tokio::test]
async fn signature_help_scopes_method_to_receiver_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_signature_help(
            r#"<?php
class Logger { public function send(string $line): void {} }
class Mailer { public function send(string $to, string $body): void {} }
$mailer = new Mailer();
$mailer->send($0);
"#,
        )
        .await;
    expect!["▶ send(string $to, string $body)  @param0"].assert_eq(&out);
}

#[tokio::test]
async fn namespaced_receiver_gets_named_argument_completion() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
namespace App;
class Mailer { public function send(string $to, string $body): void {} }
function f(Mailer $m) { $m->send($0); }
"#,
        )
        .await;
    let named: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("to:") || l.contains("body:"))
        .collect();
    expect!["Method      send(to:, body:) | named args"].assert_eq(&named.join("\n"));
}

#[tokio::test]
async fn imported_static_receiver_gets_named_argument_completion() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"//- /Mailer.php
<?php
namespace Lib;
class Mailer { public static function send(string $to, string $body): void {} }

//- /main.php
<?php
use Lib\Mailer;
Mailer::send($0);
"#,
        )
        .await;
    let named: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("to:") || l.contains("body:"))
        .collect();
    expect!["Method      send(to:, body:) | named args"].assert_eq(&named.join("\n"));
}
