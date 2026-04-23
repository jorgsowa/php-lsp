mod common;

use common::TestServer;

fn titles(resp: &serde_json::Value) -> Vec<String> {
    resp["result"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a["title"].as_str().map(str::to_owned))
        .collect()
}

#[tokio::test]
async fn code_action_phpdoc_offered_for_undocumented_function() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ca_phpdoc.php",
            "<?php\nfunction noDoc(int $x): int { return $x; }\n",
        )
        .await;

    let resp = server.code_action("ca_phpdoc.php", 1, 9, 1, 14).await;

    assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
    let t = titles(&resp);
    let has_phpdoc = t.iter().any(|s| s.to_lowercase().contains("phpdoc"));
    assert!(has_phpdoc, "expected a PHPDoc action, got: {:?}", t);
}

#[tokio::test]
async fn code_action_extract_variable_offered_on_expression() {
    let mut server = TestServer::new().await;
    server
        .open("ca_extract.php", "<?php\n$result = 1 + 2;\n")
        .await;

    let resp = server.code_action("ca_extract.php", 1, 10, 1, 15).await;

    assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
    let t = titles(&resp);
    let has_extract = t.iter().any(|s| s.to_lowercase().contains("extract"));
    assert!(has_extract, "expected an Extract action, got: {:?}", t);
}

#[tokio::test]
async fn code_action_generate_constructor_offered_for_class() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ca_ctor.php",
            "<?php\nclass Point {\n    public int $x;\n    public int $y;\n}\n",
        )
        .await;

    let resp = server.code_action("ca_ctor.php", 1, 6, 1, 11).await;

    assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
    let t = titles(&resp);
    let has_ctor = t.iter().any(|s| s.to_lowercase().contains("constructor"));
    assert!(
        has_ctor,
        "expected a Generate constructor action, got: {:?}",
        t
    );
}

#[tokio::test]
async fn code_action_implement_missing_offered() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ca_impl.php",
            "<?php\ninterface Greetable {\n    public function greet(): string;\n}\nclass Hello implements Greetable {\n}\n",
        )
        .await;

    let resp = server.code_action("ca_impl.php", 4, 0, 4, 0).await;

    assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
    let t = titles(&resp);
    let has_impl = t.iter().any(|s| s.to_lowercase().contains("implement"));
    assert!(has_impl, "expected an Implement action, got: {:?}", t);
}

#[tokio::test]
async fn code_action_add_return_type_offered() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ca_rettype.php",
            "<?php\nfunction noReturn() { return 42; }\n",
        )
        .await;

    let resp = server.code_action("ca_rettype.php", 1, 9, 1, 17).await;

    assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
    let t = titles(&resp);
    let has_ret = t.iter().any(|s| s.to_lowercase().contains("return type"));
    assert!(has_ret, "expected an Add return type action, got: {:?}", t);
}
