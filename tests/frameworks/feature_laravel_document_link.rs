//! Protocol-wired tests for document links over Laravel string-key calls,
//! against a synthetic minimal Laravel project covering every domain.

use super::*;

use expect_test::expect;
use serde_json::Value;

fn write_full_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::write(root.join(".env"), "APP_NAME=Test\n").unwrap();
    std::fs::create_dir_all(root.join("public")).unwrap();
    std::fs::write(root.join("public").join("app.js"), "x").unwrap();
    std::fs::create_dir_all(root.join("bootstrap")).unwrap();
    std::fs::write(
        root.join("bootstrap").join("app.php"),
        "<?php\n$middleware->alias(['auth' => \\App\\Http\\Middleware\\Authenticate::class]);\n",
    )
    .unwrap();
}

fn render_document_links(result: &Value, root_uri: &str) -> String {
    let links = result.as_array().cloned().unwrap_or_default();
    if links.is_empty() {
        return "<no links>".to_owned();
    }
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let mut rows: Vec<String> = links
        .iter()
        .map(|l| {
            let sl = l["range"]["start"]["line"].as_u64().unwrap_or(0);
            let sc = l["range"]["start"]["character"].as_u64().unwrap_or(0);
            let ec = l["range"]["end"]["character"].as_u64().unwrap_or(0);
            let target = l["target"].as_str().unwrap_or("<no target>");
            let target = target.strip_prefix(&prefix).unwrap_or(target);
            format!("{sl}:{sc}-{ec} target={target}")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

#[tokio::test]
async fn document_link_resolves_known_laravel_string_keys() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\nenv('APP_NAME');\nasset('app.js');\nRoute::get('/x', Foo::class)->middleware('auth');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.document_link("app.php").await;
    assert!(resp["error"].is_null(), "error: {resp:?}");
    let out = render_document_links(&resp["result"], &s.uri(""));
    expect![[r#"
        1:5-13 target=.env
        2:7-13 target=public/app.js
        3:42-46 target=bootstrap/app.php"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn document_link_skips_unresolved_laravel_keys() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\nenv('MISSING_KEY');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.document_link("app.php").await;
    let out = render_document_links(&resp["result"], &s.uri(""));
    expect!["<no links>"].assert_eq(&out);
}

#[tokio::test]
async fn document_link_empty_outside_laravel_project() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    // No `artisan` marker — plain PHP project, even with a matching `.env`.
    std::fs::write(workspace.path().join(".env"), "APP_NAME=Test\n").unwrap();
    let php = "<?php\nenv('APP_NAME');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.document_link("app.php").await;
    let out = render_document_links(&resp["result"], &s.uri(""));
    expect!["<no links>"].assert_eq(&out);
}
