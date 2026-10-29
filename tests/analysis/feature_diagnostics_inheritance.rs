//! Diagnostic coverage matrix using the caret annotation DSL.
//! Each test names the expectation inline with `// ^^^ severity: message`.

use super::*;

use expect_test::expect;
use serde_json::json;

#[tokio::test]
async fn circular_inheritance_suppressed_when_type_errors_disabled() {
    let (mut s, _resp) = TestServer::new_with_options(json!({
        "diagnostics": { "typeErrors": false }
    }))
    .await;
    s.check_diagnostics(
        r#"<?php
class A extends A {}
"#,
    )
    .await;
}

#[tokio::test]
async fn same_namespace_trait_use_truly_missing_is_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("composer.json"),
        r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
    )
    .unwrap();

    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    let user_src = "<?php\nnamespace App;\nclass Person {\n    use MissingTrait;\n}\n";
    std::fs::write(tmp.path().join("src/Person.php"), user_src).unwrap();

    // Whether `MissingTrait` is truly undefined or just not indexed yet is
    // indistinguishable until the scan finishes, so wait for it — see
    // `compute_open_file_diagnostics` and issue #242.
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("src/Person.php", user_src).await;

    let resp = s.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &s.uri(""));
    expect![[r#"
        src/Person.php
          3:8 Trait MissingTrait does not exist [UndefinedTrait] (error)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn workspace_diagnostic_circular_inheritance() {
    let mut server = TestServer::new().await;
    server
        .open(
            "ws_circular.php",
            "<?php\nclass A extends B {}\nclass B extends A {}\n",
        )
        .await;

    let resp = server.workspace_diagnostic().await;
    let out = render_workspace_diagnostic(&resp, &server.uri(""));

    expect![[r#"
        ws_circular.php
          2:0 Class B has a circular inheritance chain [CircularInheritance] (error)"#]]
    .assert_eq(&out);
}

// ── Abstract method enforcement ──────────────────────────────────────────────
