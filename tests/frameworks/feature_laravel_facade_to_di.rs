//! Regression tests for the "Convert facade call to dependency injection"
//! quickfix (`src/actions/facade_to_di_action.rs`).

use super::*;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
}

const TITLE: &str = "Convert Cache:: call to dependency injection";

#[tokio::test]
async fn generates_constructor_when_none_exists() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let out = s
        .check_code_action_apply(
            r#"<?php
class Report {
    public function build(): array {
        return Cache$0::get('key');
    }
}
"#,
            TITLE,
        )
        .await;

    assert!(
        out.contains("$this->cache->get('key')"),
        "call site should be rewritten, got:\n{out}"
    );
    assert!(
        out.contains(
            "public function __construct(private \\Illuminate\\Contracts\\Cache\\Repository $cache)"
        ),
        "a constructor should be generated with the promoted param, got:\n{out}"
    );
}

#[tokio::test]
async fn appends_param_to_existing_constructor() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let out = s
        .check_code_action_apply(
            r#"<?php
class Report {
    public function __construct(private Logger $logger) {
    }
    public function build(): array {
        return Cache$0::get('key');
    }
}
"#,
            TITLE,
        )
        .await;

    assert!(
        out.contains("$this->cache->get('key')"),
        "call site should be rewritten, got:\n{out}"
    );
    assert!(
        out.contains(
            "private Logger $logger, private \\Illuminate\\Contracts\\Cache\\Repository $cache"
        ),
        "the new param should be appended after the existing one, got:\n{out}"
    );
}

#[tokio::test]
async fn not_offered_in_static_method() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "report.php",
        "<?php\nclass Report {\n    public static function build(): array {\n        return Cache::get('key');\n    }\n}\n",
    )
    .await;

    let resp = s.code_action("report.php", 3, 17, 3, 17).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions.iter().any(|a| a["title"].as_str() == Some(TITLE)),
        "a static method has no $this to inject into — must not be offered"
    );
}

#[tokio::test]
async fn not_offered_when_property_already_named_after_facade() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "report.php",
        "<?php\nclass Report {\n    private $cache;\n    public function build(): array {\n        return Cache::get('key');\n    }\n}\n",
    )
    .await;

    let resp = s.code_action("report.php", 4, 17, 4, 17).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions.iter().any(|a| a["title"].as_str() == Some(TITLE)),
        "a class that already has a $cache property must not get a conflicting second injection"
    );
}

#[tokio::test]
async fn not_offered_for_unrelated_static_call() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "report.php",
        "<?php\nclass Report {\n    public function build(): array {\n        return SomeUnrelatedClass::get('key');\n    }\n}\n",
    )
    .await;

    let resp = s.code_action("report.php", 3, 30, 3, 30).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions.iter().any(|a| a["title"]
            .as_str()
            .is_some_and(|t| t.contains("dependency injection"))),
        "a non-facade static call must not offer this quickfix"
    );
}

#[tokio::test]
async fn not_offered_in_non_laravel_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    // Deliberately no `artisan` — not a Laravel workspace.
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "report.php",
        "<?php\nclass Report {\n    public function build(): array {\n        return Cache::get('key');\n    }\n}\n",
    )
    .await;

    let resp = s.code_action("report.php", 3, 17, 3, 17).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions.iter().any(|a| a["title"].as_str() == Some(TITLE)),
        "non-Laravel workspaces must never offer this quickfix"
    );
}
