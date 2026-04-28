mod common;

use common::{TestServer, render_inlay_hints};
use expect_test::expect;

/// The definition file is never opened — it exists only in the workspace index
/// from the background scan. This is the typical production scenario.
#[tokio::test]
async fn inlay_hints_from_workspace_index_only() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("greeter.php"),
        "<?php\nfunction greet(string $name, int $count): void {}\n",
    )
    .unwrap();
    let caller_src = "<?php\ngreet('world', 3);\n";
    std::fs::write(tmp.path().join("caller.php"), caller_src).unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    // Only open the caller — greeter.php is indexed but never opened.
    s.open("caller.php", caller_src).await;
    let resp = s.inlay_hints("caller.php", 0, 0, 3, 0).await;
    expect![[r#"
        1:6 name:
        1:15 count:"#]]
    .assert_eq(&render_inlay_hints(&resp));
}

#[tokio::test]
async fn inlay_hints_cross_file_function_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
greet('world', 3);

//- /greeter.php
<?php
function greet(string $name, int $count): void {}
"#,
        )
        .await;
    expect![[r#"
        1:6 name:
        1:15 count:"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_cross_file_constructor_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
$p = new Point(1, 2);

//- /Point.php
<?php
class Point {
    public function __construct(int $x, int $y) {}
}
"#,
        )
        .await;
    expect![[r#"
        1:15 x:
        1:18 y:"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_cross_file_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
$g = new Greeter();
$g->sayHello('World');

//- /Greeter.php
<?php
class Greeter {
    public function sayHello(string $name): void {}
}
"#,
        )
        .await;
    expect![[r#"
        2:13 name:"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_for_parameter_names() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
function greet(string $name, int $count): void {}
greet('world', 3);
"#,
        )
        .await;
    expect![[r#"
        2:6 name:
        2:15 count:"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hint_resolve_returns_same_hint() {
    let mut s = TestServer::new().await;
    s.open(
        "resolve.php",
        "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
    )
    .await;
    let hints_resp = s.inlay_hints("resolve.php", 0, 0, 4, 0).await;
    let hints = hints_resp["result"].as_array().cloned().unwrap_or_default();
    assert!(!hints.is_empty(), "expected inlay hints");
    let resp = s.inlay_hint_resolve(hints[0].clone()).await;
    assert!(resp["error"].is_null(), "inlayHint/resolve error: {resp:?}");
    assert_eq!(
        resp["result"]["label"], hints[0]["label"],
        "resolved label must match original"
    );
}

#[tokio::test]
async fn inlay_hints_nullsafe_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
$g = new Greeter();
$g?->sayHello('World');

//- /Greeter.php
<?php
class Greeter {
    public function sayHello(string $name): void {}
}
"#,
        )
        .await;
    expect![[r#"
        2:14 name:"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_static_method_call() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"//- /caller.php
<?php
Greeter::sayHello('world');

//- /Greeter.php
<?php
class Greeter {
    public static function sayHello(string $name): void {}
}
"#,
        )
        .await;
    expect![[r#"
        1:18 name:"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn inlay_hints_empty_for_file_with_no_calls() {
    let mut s = TestServer::new().await;
    let out = s
        .check_inlay_hints(
            r#"<?php
$x = 1;
$y = 2;
"#,
        )
        .await;
    expect!["<no hints>"].assert_eq(&out);
}
