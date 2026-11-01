//! Protocol-wired tests for Laravel `config('a.b.c')` go-to-definition and
//! completion, against a synthetic minimal Laravel project.

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config").join("database.php"),
        "<?php\nreturn [\n    'default' => 'mysql',\n    'connections' => [\n        'mysql' => [\n            'host' => '127.0.0.1',\n        ],\n    ],\n];\n",
    )
    .unwrap();
}

#[tokio::test]
async fn config_call_goto_definition_resolves_top_level_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$driver = config('database.default');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 26 = inside "database.default".
    let resp = s.definition("app.php", 1, 26).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["config/database.php:2:5-2:12"].assert_eq(&out);
}

#[tokio::test]
async fn config_call_goto_definition_resolves_nested_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$host = config('database.connections.mysql.host');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 45 = inside the dotted key, on "host".
    let resp = s.definition("app.php", 1, 45).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["config/database.php:5:13-5:17"].assert_eq(&out);
}

#[tokio::test]
async fn config_call_completion_lists_dotted_keys_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nconfig('database.c\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 19 = right after "database.c".
    let resp = s.completion("app.php", 1, 19).await;
    let out = render_completion(&resp);
    expect![[r#"
        Property    database.connections
        Property    database.connections.mysql
        Property    database.connections.mysql.host"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn config_call_goto_definition_resolves_key_in_non_utf8_file() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::create_dir_all(workspace.path().join("config")).unwrap();
    std::fs::write(
        workspace.path().join("config").join("app.php"),
        b"<?php\n// Caf\xe9\nreturn [\n    'name' => 'Caf\xe9',\n];\n",
    )
    .unwrap();
    let php = "<?php\n$name = config('app.name');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.definition("app.php", 1, 23).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["config/app.php:3:5-3:9"].assert_eq(&out);
}

#[tokio::test]
async fn laravel_app_in_second_workspace_root_is_indexed() {
    use serde_json::json;
    let lib = tempfile::tempdir().expect("lib tempdir");
    std::fs::write(lib.path().join("Util.php"), "<?php\nclass Util {}\n").unwrap();
    let app = tempfile::tempdir().expect("app tempdir");
    write_minimal_laravel_project(app.path());
    let php = "<?php\n$driver = config('database.default');\n";
    std::fs::write(app.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_roots(&[lib.path(), app.path()]).await;
    s.wait_for_index_ready().await;
    let app_uri = tower_lsp_server::ls_types::Uri::from_file_path(app.path()).unwrap();
    let uri = format!("{}/app.php", app_uri.as_str());
    let client = s.client();
    client
        .notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "php", "version": 1, "text": php}}),
        )
        .await;
    client.wait_for_diagnostics_version(&uri, 1).await;
    let at = json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": 26}});

    let resp = client.request("textDocument/definition", at.clone()).await;
    expect!["config/database.php:2:5-2:12"].assert_eq(&render_locations(&resp, app_uri.as_str()));
    // Hover shows the definition path relative to the Laravel app's root.
    let resp = client.request("textDocument/hover", at).await;
    expect![[r#"
        **config('database.default')**

        ```php
        'default' => 'mysql',
        ```

        `config/database.php`"#]]
    .assert_eq(&render_hover(&resp));
}
