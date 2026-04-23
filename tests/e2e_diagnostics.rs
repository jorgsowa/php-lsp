mod common;

use common::TestServer;

fn has_code(notif: &serde_json::Value, code: &str) -> bool {
    notif["params"]["diagnostics"]
        .as_array()
        .map(|arr| arr.iter().any(|d| d["code"].as_str() == Some(code)))
        .unwrap_or(false)
}

#[tokio::test]
async fn diagnostics_published_on_did_open_for_undefined_function() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("diag_test.php", "<?php\nnonexistent_function();\n")
        .await;

    assert!(
        has_code(&notif, "UndefinedFunction"),
        "expected UndefinedFunction in diagnostics: {:?}",
        notif["params"]["diagnostics"],
    );
}

#[tokio::test]
async fn diagnostics_published_on_did_change_for_undefined_function() {
    let mut server = TestServer::new().await;
    server.open("change_test.php", "<?php\n").await;

    let notif = server
        .change("change_test.php", 2, "<?php\nnonexistent_function();\n")
        .await;

    assert!(
        has_code(&notif, "UndefinedFunction"),
        "expected UndefinedFunction after didChange: {:?}",
        notif["params"]["diagnostics"],
    );
}

#[tokio::test]
async fn did_open_emits_diagnostic_for_undefined_class() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("undef_class.php", "<?php\n$x = new UnknownClass();\n")
        .await;

    assert!(
        has_code(&notif, "UndefinedClass"),
        "expected UndefinedClass diagnostic on did_open, got: {:?}",
        notif["params"]["diagnostics"]
    );
}

#[tokio::test]
async fn diagnostics_clear_when_code_is_fixed() {
    let mut server = TestServer::new().await;
    let notif = server
        .open("fix_test.php", "<?php\nnonexistent_function();\n")
        .await;
    assert!(
        !notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "expected at least one diagnostic for broken code, got: {:?}",
        notif["params"]["diagnostics"]
    );

    let notif = server.change("fix_test.php", 2, "<?php\n").await;
    let diags = notif["params"]["diagnostics"]
        .as_array()
        .unwrap_or(&vec![])
        .clone();
    assert!(
        diags.is_empty(),
        "diagnostics must be empty after fixing the code, got: {:?}",
        diags
    );
}

#[tokio::test]
async fn mir_analyzer_scope_of_undefined_function_detection() {
    const TOP_LEVEL: &str = "<?php\nnonexistent_function();\n";
    const IN_FUNCTION: &str = "<?php\nfunction f(): void {\n    nonexistent_function();\n}\n";
    const IN_METHOD: &str = "<?php\nclass A {\n    public function f(): void {\n        nonexistent_function();\n    }\n}\n";
    const IN_NAMESPACED_METHOD: &str = "<?php\nnamespace LspTest;\nclass Broken {\n    public function f(): void {\n        nonexistent_function();\n    }\n}\n";

    async fn codes(server: &mut TestServer, path: &str, text: &str) -> Vec<String> {
        let notif = server.open(path, text).await;
        let empty = vec![];
        notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|d| d["code"].as_str().map(str::to_owned))
            .collect()
    }

    let mut server = TestServer::new().await;

    let top_level = codes(&mut server, "scope1.php", TOP_LEVEL).await;
    assert!(
        top_level.contains(&"UndefinedFunction".to_owned()),
        "top-level call: expected UndefinedFunction, got: {:?}",
        top_level
    );

    let in_function = codes(&mut server, "scope2.php", IN_FUNCTION).await;
    assert!(
        in_function.contains(&"UndefinedFunction".to_owned()),
        "call inside plain function: expected UndefinedFunction, got: {:?}",
        in_function
    );

    let in_method = codes(&mut server, "scope3.php", IN_METHOD).await;
    assert!(
        in_method.contains(&"UndefinedFunction".to_owned()),
        "call inside class method: expected UndefinedFunction, got: {:?}",
        in_method
    );

    let in_namespaced_method = codes(&mut server, "scope4.php", IN_NAMESPACED_METHOD).await;
    assert!(
        in_namespaced_method.contains(&"UndefinedFunction".to_owned()),
        "call inside namespaced class method: expected UndefinedFunction, got: {:?}",
        in_namespaced_method
    );
}

#[tokio::test]
async fn issue_170_undefined_function_in_method_body_is_detected() {
    const ISSUE_170_PHP: &str = r#"<?php
namespace LspTest;

class Broken
{
    public int $count = 0;

    public function bump(): int
    {
        $this->count++;
        return $this->count;
    }

    public function obviouslyBroken(): int
    {
        nonexistent_function();
        $x = new UnknownClass();
        return 0;
    }
}
"#;

    let mut server = TestServer::new().await;
    let notif = server.open("issue170.php", ISSUE_170_PHP).await;
    let empty = vec![];
    let codes: Vec<String> = notif["params"]["diagnostics"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|d| d["code"].as_str().map(str::to_owned))
        .collect();

    assert!(
        codes.contains(&"UndefinedFunction".to_owned()),
        "expected UndefinedFunction inside a namespaced class method, got: {:?}",
        codes
    );
    assert!(
        codes.contains(&"UndefinedClass".to_owned()),
        "expected UndefinedClass inside a namespaced class method, got: {:?}",
        codes
    );
}
