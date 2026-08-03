//! Comprehensive hover coverage.

use super::*;

use expect_test::expect;

#[tokio::test]
async fn hover_across_files_via_use() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"//- /src/Greeter.php
<?php
namespace App;
class Greeter {
    public function hello(): string { return 'hi'; }
}

//- /src/main.php
<?php
use App\Greeter;
$g = new Greeter();
$g->hel$0lo();
"#,
        expect![[r#"
            ```php
            Greeter::hello(): string
            ```"#]],
    )
    .await;
}

#[tokio::test]
async fn hover_class_as_param_type_cross_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Post.php"),
        "<?php\nclass Post { public string $title = ''; }\n",
    )
    .unwrap();
    let ctrl_src = "<?php\nfunction show(Post $post): void {}\n";
    std::fs::write(tmp.path().join("Controller.php"), ctrl_src).unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    // Only open Controller.php — Post.php is indexed but never opened.
    s.open("Controller.php", ctrl_src).await;
    let (_, line, col) = s.locate("Controller.php", "Post", 0);
    let resp = s.hover("Controller.php", line, col).await;
    expect![[r#"
        ```php
        class Post
        ```"#]]
    .assert_eq(&render_hover(&resp));
}

#[tokio::test]
async fn hover_class_in_extends_clause_cross_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Base.php"), "<?php\nclass Base {}\n").unwrap();
    let child_src = "<?php\nclass Child extends Base {}\n";
    std::fs::write(tmp.path().join("Child.php"), child_src).unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    // Only open Child.php — Base.php is indexed but never opened.
    s.open("Child.php", child_src).await;
    let (_, line, col) = s.locate("Child.php", "Base", 0);
    let resp = s.hover("Child.php", line, col).await;
    expect![[r#"
        ```php
        class Base
        ```"#]]
    .assert_eq(&render_hover(&resp));
}

#[tokio::test]
async fn hover_static_property_cross_file() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"//- /caller.php
<?php
Config::$ver$0sion;

//- /Config.php
<?php
class Config {
    public static string $version = '1.0';
}
"#,
        expect![[r#"
            ```php
            (property) public static Config::$version: string
            ```"#]],
    )
    .await;
}

// ── 1.3 First-class callable hover ──────────────────────────────────────────

#[tokio::test]
async fn hover_use_alias_resolves_to_class() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"<?php
class Mailer { public function send(): void {} }
use Mailer as Sender;
$s = new Send$0er();
"#,
        expect![[r#"
            ```php
            class Mailer
            ```"#]],
    )
    .await;
}

/// Companion to `hover_use_alias_resolves_to_class` above: when the aliased
/// import's *pre-alias* short name collides with the short name of the class
/// doing the importing (different namespaces, different files — only the
/// bare identifier matches), hover on the alias's usage misattributes to the
/// enclosing class instead of the actual aliased target.
#[tokio::test]
#[ignore = "known bug: hover on an aliased-import usage shows the enclosing \
            class's own card when the alias's pre-alias short name collides \
            with the enclosing class's short name — goto-definition resolves \
            correctly through this same alias, only hover is affected"]
async fn hover_use_alias_misattributes_on_short_name_collision() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    s.check_hover_annotated(
        r#"//- /src/Vendor/Widget.php
<?php
namespace Vendor;
/** The real aliased target — hover should show this docblock. */
class Widget { public function render(): void {} }

//- /src/App/Consumer.php
<?php
namespace App;
use Vendor\Widget as Renderer;
/** The enclosing class — hover must NOT show this docblock here. */
class Widget {
    public function make(): void {
        $x = new Rende$0rer();
    }
}
"#,
        expect![[r#"
            ```php
            class Widget
            ```

            ---

            The enclosing class — hover must NOT show this docblock here."#]],
    )
    .await;
}
