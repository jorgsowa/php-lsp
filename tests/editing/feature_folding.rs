use super::*;

use expect_test::expect;
use serde_json::{Value, json};

#[tokio::test]
async fn folding_interface() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
interface Countable {
    public function count(): int;
}
"#,
        )
        .await;
    expect!["1..3 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_trait_and_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
trait Loggable {
    public function log(): void {
        echo 'log';
    }
}
"#,
        )
        .await;
    expect![[r#"
        1..5 region
        2..4 region"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn folding_braced_namespace() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
namespace App {
    class Foo {}
}
"#,
        )
        .await;
    expect!["1..3 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_single_line_construct_produces_no_fold() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s.check_folding("<?php\nclass Inline {}\n").await;
    expect!["<no folds>"].assert_eq(&out);
}

#[tokio::test]
async fn folding_empty_file_produces_no_fold() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s.check_folding("<?php\n").await;
    expect!["<no folds>"].assert_eq(&out);
}

#[tokio::test]
async fn folding_if_statement() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
if (true) {
    echo 'yes';
}
"#,
        )
        .await;
    expect!["1..3 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_foreach_statement() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
foreach ([1, 2, 3] as $i) {
    echo $i;
}
"#,
        )
        .await;
    expect!["1..3 region"].assert_eq(&out);
}

/// `switch` fell into fold_stmt's catch-all `_ => {}` arm — no fold range
/// was ever produced for it, however long the body.
#[tokio::test]
async fn folding_switch_statement() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
switch ($x) {
    case 1:
        foo();
        break;
    case 2:
        bar();
        break;
}
"#,
        )
        .await;
    expect!["1..8 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_try_catch() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
try {
    risky();
} catch (\Exception $e) {
    echo 'error';
}
"#,
        )
        .await;
    expect!["1..5 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_multiline_doc_comment() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
/**
 * A documented function.
 * @param int $x
 */
function foo(int $x): void {}
"#,
        )
        .await;
    expect!["1..4 comment"].assert_eq(&out);
}

#[tokio::test]
async fn folding_region_endregion() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
// #region MySection
$a = 1;
// #endregion
"#,
        )
        .await;
    expect!["1..3 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_consecutive_use_statements() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
use A\ClassA;
use B\ClassB;
use C\ClassC;
"#,
        )
        .await;
    expect!["1..3 imports"].assert_eq(&out);
}

#[tokio::test]
async fn folding_nested_constructs_both_returned() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
class Container {
    public function work(): void {
        if (true) {
            echo 'x';
        }
    }
}
"#,
        )
        .await;
    expect![[r#"
        1..7 region
        2..6 region
        3..5 region"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn folding_single_line_function_not_folded() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding("<?php\nfunction tiny(): void { echo 1; }\n")
        .await;
    expect!["<no folds>"].assert_eq(&out);
}

#[tokio::test]
async fn folding_enum_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
enum Status {
    case Active;
    public function label(): string {
        return 'active';
    }
}
"#,
        )
        .await;
    expect![[r#"
        1..6 region
        3..5 region"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn folding_ranges_cover_function_body() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
function f(): void {
    $a = 1;
    $b = 2;
    $c = 3;
}
"#,
        )
        .await;
    expect!["1..5 region"].assert_eq(&out);
}

#[tokio::test]
async fn folding_ranges_cover_class_and_method() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_folding(
            r#"<?php
class Folded {
    public function method(): void {
        // body
    }
}
"#,
        )
        .await;
    expect![[r#"
        1..5 region
        2..4 region"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn code_lens_for_function_with_reference() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_lens(
            r#"<?php
function lensed(): void {}
lensed();
"#,
        )
        .await;
    expect!["L1:9-L1:15: 1 reference [editor.action.showReferences]"].assert_eq(&out);
}

#[tokio::test]
async fn code_lens_for_class_with_references() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_lens(
            r#"<?php
class Widget {}
$w = new Widget();
"#,
        )
        .await;
    expect!["L1:6-L1:12: 1 reference [editor.action.showReferences]"].assert_eq(&out);
}

fn render_resolved_lens(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let l = &resp["result"];
    let sl = l["range"]["start"]["line"].as_u64().unwrap_or(0);
    let title = l["command"]["title"].as_str().unwrap_or("<unresolved>");
    let cmd = l["command"]["command"].as_str().unwrap_or("");
    let data = if l.get("data").map(|d| !d.is_null()).unwrap_or(false) {
        format!(" data={}", l["data"])
    } else {
        String::new()
    };
    format!("L{sl}: {title} [{cmd}]{data}")
}

#[tokio::test]
async fn code_lens_resolve_round_trips_real_lens() {
    let mut server = TestServer::new().await;
    server
        .open("lens.php", "<?php\nfunction lensed(): void {}\nlensed();\n")
        .await;

    let lens = server.code_lens("lens.php").await["result"][0].clone();
    assert!(lens.is_object(), "expected at least one code lens");

    let resp = server.client().request("codeLens/resolve", lens).await;
    expect!["L1: 1 reference [editor.action.showReferences]"]
        .assert_eq(&render_resolved_lens(&resp));
}

#[tokio::test]
async fn code_lens_resolve_preserves_command_and_data() {
    let mut server = TestServer::new().await;
    let lens = json!({
        "range": {
            "start": { "line": 7, "character": 0 },
            "end":   { "line": 7, "character": 1 }
        },
        "command": {
            "title": "synthetic",
            "command": "noop",
            "arguments": [42]
        },
        "data": { "marker": "keep-me" }
    });

    let resp = server.client().request("codeLens/resolve", lens).await;
    expect![[r#"L7: synthetic [noop] data={"marker":"keep-me"}"#]]
        .assert_eq(&render_resolved_lens(&resp));
}
