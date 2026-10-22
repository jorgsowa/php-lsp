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

#[tokio::test]
async fn docblock_property_and_method_tags_are_offered() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
/**
 * @property string $email
 * @property-read int $id
 * @method int total()
 * @method static self make()
 */
class User {}
function f(User $u) { $u->$0 }
"#,
        )
        .await;
    expect![[r#"
        Property    $email
        Property    $id
        Method      total"#]]
    .assert_eq(&members(&out));
}

#[tokio::test]
async fn mixin_public_members_are_offered() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
class Builder {
    public function where(): static { return $this; }
    protected function guarded(): void {}
    public static function query(): void {}
}
/** @mixin Builder */
class Model { public function save(): void {} }
function f(Model $m) { $m->$0 }
"#,
        )
        .await;
    expect![[r#"
        Method      save
        Method      where"#]]
    .assert_eq(&members(&out));
}

#[tokio::test]
async fn private_trait_members_are_offered_inside_the_using_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
trait Helpers {
    private function helper(): void {}
}
class Base {
    use Helpers;
}
class Child extends Base {
    use Helpers2;
    public function run(): void { $this->$0 }
}
trait Helpers2 {
    private function own(): void {}
}
"#,
        )
        .await;
    expect![[r#"
        Method      own
        Method      run"#]]
    .assert_eq(&members(&out));
}

#[tokio::test]
async fn interface_constants_are_offered_on_implementing_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
namespace App;
interface HasVersion { const VERSION = 1; }
class Release implements HasVersion { const NAME = 'x'; }
Release::$0
"#,
        )
        .await;
    expect![[r#"
        Constant    NAME
        Constant    VERSION"#]]
    .assert_eq(&members(&out));
}

#[tokio::test]
async fn backed_enum_static_access_offers_cases_and_builtins() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_completion_ordered(
            r#"<?php
namespace App;
enum Suit: string {
    case Hearts = 'h';
    const DEFAULT = self::Hearts;
    public static function random(): self { return self::Hearts; }
}
Suit::$0
"#,
        )
        .await;
    expect![[r#"
        Constant    DEFAULT
        Constant    Hearts
        Method      cases
        Method      from
        Method      random
        Method      tryFrom"#]]
    .assert_eq(&members(&out));
}

/// Unopened PSR-4 vendor classes: traits and the grandparent of a vendor
/// parent must all contribute members.
#[tokio::test]
async fn vendor_parent_traits_and_grandparent_members_are_offered() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("vendor/acme/console/src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"Acme\\Console\\":"vendor/acme/console/src/"}}}"#,
    )
    .unwrap();
    std::fs::write(
        src.join("BaseCommand.php"),
        "<?php\nnamespace Acme\\Console;\nclass BaseCommand { public function run(): int { return 0; } }\n",
    )
    .unwrap();
    std::fs::write(
        src.join("InteractsWithIO.php"),
        "<?php\nnamespace Acme\\Console;\ntrait InteractsWithIO { public function line(): void {} }\n",
    )
    .unwrap();
    std::fs::write(
        src.join("CallsCommands.php"),
        "<?php\nnamespace Acme\\Console;\ntrait CallsCommands { protected function call(): void {} }\n",
    )
    .unwrap();
    std::fs::write(
        src.join("Command.php"),
        "<?php\nnamespace Acme\\Console;\nclass Command extends BaseCommand {\n    use InteractsWithIO, CallsCommands;\n    private function secret(): void {}\n}\n",
    )
    .unwrap();
    let caller = "<?php\nnamespace App;\nuse Acme\\Console\\Command;\nclass Sync extends Command {\n    public function handle(): void {\n        $this->\n    }\n}\n";
    std::fs::write(tmp.path().join("Sync.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("Sync.php", caller).await;

    let (_, line, ch) = s.locate("Sync.php", "$this->", 0);
    let resp = s
        .completion("Sync.php", line, ch + "$this->".len() as u32)
        .await;
    let out = render_completion_ordered(&resp);
    expect![[r#"
        Method      call
        Method      handle
        Method      line
        Method      run"#]]
    .assert_eq(&members(&out));
}
