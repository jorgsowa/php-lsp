//! Hover/references/definition with the cursor inside a `use` import line
//! itself (not on a usage site). Hover echoes the raw `use` line back as
//! text; references should still resolve across the import and declaration.
//! Coverage for the actual willRenameFiles/willDeleteFiles use-statement
//! rewrite lives in feature_file_ops.rs.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn simple_use_statement_resolves() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let hover = s
        .check_hover(
            r#"<?php
use App$0\Services\Foo;

class Foo {}
"#,
        )
        .await;
    // Use statement namespace should be recognized
    expect![[r#"`use App\Services\Foo;`"#]].assert_eq(&hover);
}

#[tokio::test]
async fn use_with_leading_backslash() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
use \App$0\Services\OldService;

echo 'ok';
"#,
        )
        .await;
    // The use statement with leading backslash should be recognized
    expect![[r#"`use \App\Services\OldService;`"#]].assert_eq(&hover);
}

#[tokio::test]
async fn aliased_import_cursor_finds_references() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let refs = s
        .check_references(
            r#"//- /main.php
<?php
use App$0\Services\OldService as Service;

//- /App/Services/OldService.php
<?php
namespace App\Services;
class OldService {}
"#,
        )
        .await;
    expect!["App/Services/OldService.php:2:6-2:16\nmain.php:1:4-1:38"]
    .assert_eq(&refs);
}

#[tokio::test]
async fn partial_class_names_not_matched() {
    let mut s = TestServer::new().await;
    let refs = s
        .check_references(
            r#"<?php
use App$0\Services\Foo;
class FooExtra {}

$x = new FooExtra();
"#,
        )
        .await;
    // The import itself is a reference, but like-named classes must not match.
    expect!["main.php:1:4-1:20"].assert_eq(&refs);
}

#[tokio::test]
async fn non_use_lines_ignored() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
// use App\Old\Class;
$x = new App$0\Old\Class();
"#,
        )
        .await;
    // Direct instantiation should be recognized even with commented use
    expect!["<no hover>"].assert_eq(&hover);
}

#[tokio::test]
async fn namespace_and_use_together() {
    let mut s = TestServer::new().await;
    let hover = s
        .check_hover(
            r#"<?php
namespace App;
use Services$0\OldName;
"#,
        )
        .await;
    // Both namespace and use should be parsed together
    expect![[r#"`use Services\OldName;`"#]].assert_eq(&hover);
}

#[tokio::test]
async fn function_imports_resolved() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let hover = s
        .check_hover(
            r#"<?php
use function App$0\helper;
use const App\LIMIT;
use App\Class;

class Class {}
"#,
        )
        .await;
    // Function imports should be recognized
    expect![[r#"`use function App\helper;`"#]].assert_eq(&hover);
}

#[tokio::test]
async fn references_across_use_statements() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    // References should find both the import and the class usage.
    s.check_references_annotated(
        r#"<?php
use App$0\Logger;
//  ^^^^^^^^^^ ref

class Logger {}

$log = new Logger();
//         ^^^^^^ ref
"#,
    )
    .await;
}

#[tokio::test]
async fn definition_on_use_statement_segment_finds_nothing() {
    let mut s = TestServer::new().await;
    let def = s
        .check_definition(
            r#"<?php
use App$0\Services\Service;

echo 'test';
"#,
        )
        .await;
    // Cursor on a `use` import segment — no definition resolves.
    expect!["<none>"].assert_eq(&def);
}

#[tokio::test]
async fn aliased_import_alias_cursor_finds_references() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let refs = s
        .check_references(
            r#"//- /main.php
<?php
use App\Se$0rvices\MyClass as MC;

//- /App/Services/MyClass.php
<?php
namespace App\Services;
class MyClass {}
"#,
        )
        .await;
    expect!["App/Services/MyClass.php:2:6-2:13\nmain.php:1:4-1:30"]
    .assert_eq(&refs);
}
