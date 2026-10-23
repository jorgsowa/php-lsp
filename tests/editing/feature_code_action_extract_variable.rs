//! Extract variable code action transformation tests.
//! Tests verify that expressions are extracted into new variables.

use super::*;
use expect_test::expect;

#[tokio::test]
async fn extract_variable_from_expression() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
function calc() {
    echo $01 + 2$0;
}
"#,
            "Extract variable",
        )
        .await;
    expect![[r#"
        <?php
        function calc() {
            $extracted = 1 + 2;
            echo $extracted;
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_variable_function_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Service {
    public function run() {
        return $0strlen("test")$0;
    }
}
"#,
            "Extract variable",
        )
        .await;
    expect![[r#"
        <?php
        class Service {
            public function run() {
                $extracted = strlen("test");
                return $extracted;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_variable_array_access() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$data = ["key" => "value"];
echo $0$data["key"]$0;
"#,
            "Extract variable",
        )
        .await;
    expect![[r#"
        <?php
        $data = ["key" => "value"];
        $extracted = $data["key"];
        echo $extracted;
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_variable_method_call() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Handler {
    public function process() {
        $result = $0$this->getValue()$0 + 10;
    }
}
"#,
            "Extract variable",
        )
        .await;
    expect![[r#"
        <?php
        class Handler {
            public function process() {
                $extracted = $this->getValue();
                $result = $extracted + 10;
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_variable_preserves_indentation_nested() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
class Service {
    public function process() {
        if (true) {
            return $01 + 2$0;
        }
    }
}
"#,
            "Extract variable",
        )
        .await;
    // Should create $extracted at the correct indentation level
    expect![[r#"
        <?php
        class Service {
            public function process() {
                if (true) {
                    $extracted = 1 + 2;
                    return $extracted;
                }
            }
        }
    "#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn extract_variable_no_action_for_empty_selection() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = foo();$0
"#,
            "Extract variable",
        )
        .await;
    expect!["<action not found: Extract variable>"].assert_eq(&out);
}

#[tokio::test]
async fn extract_variable_no_action_for_plain_variable() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let out = s
        .check_code_action_apply(
            r#"<?php
$x = $0$foo$0;
"#,
            "Extract variable",
        )
        .await;
    expect!["<action not found: Extract variable>"].assert_eq(&out);
}

#[tokio::test]
async fn extract_variable_multibyte_unicode_string() {
    // "é" is 2 UTF-8 bytes but 1 UTF-16 code unit; LSP positions are UTF-16.
    // Slicing by byte offset would either panic or return garbage.
    let mut s = TestServer::new().await;
    let out = s
        .check_code_action_apply(
            r#"<?php
function greet(): string {
    return $0"café"$0;
}
"#,
            "Extract variable",
        )
        .await;
    expect![[r#"
        <?php
        function greet(): string {
            $extracted = "café";
            return $extracted;
        }
    "#]]
    .assert_eq(&out);
}

// A watched-file delete blanks the file's salsa text; a later edit must re-attach
// the open buffer instead of leaving the AST parsed from "" (byte offsets underflow).
#[tokio::test]
async fn extract_variable_after_open_file_deleted_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("main.php"), "<?php\n$b = 3 + 4\n;\n").unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("main.php", "<?php\n$b = 3 + 4\n;\n").await;

    std::fs::remove_file(tmp.path().join("main.php")).unwrap();
    let uri = s.uri("main.php");
    s.did_change_watched_files(vec![(uri, 3)]).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !s.code_action("main.php", 1, 5, 1, 10).await["result"].is_null() {
        assert!(std::time::Instant::now() < deadline, "delete never applied");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let padded = format!("<?php\n$a = 1 + 2{}\n;\n", " ".repeat(40));
    s.change("main.php", 2, &padded).await;
    let resp = s.code_action("main.php", 1, 5, 1, 50).await;
    assert!(
        resp.to_string().contains("$extracted = 1 + 2;"),
        "extract variable must see the edited buffer, got: {resp}"
    );
}
