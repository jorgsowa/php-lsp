//! Protocol-wired tests for Laravel `env('KEY')` go-to-definition and
//! completion, wired against a synthetic minimal Laravel project (not the
//! full framework corpus in `benches/fixtures/laravel` — these only need an
//! `artisan` marker file, a `.env`, and one PHP file).

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path, env: &str, php: &str) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::write(root.join(".env"), env).unwrap();
    std::fs::write(root.join("app.php"), php).unwrap();
}

#[tokio::test]
async fn env_call_goto_definition_resolves_to_dot_env() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(
        workspace.path(),
        "APP_NAME=TestApp\nDB_HOST=127.0.0.1\n",
        "<?php\n$name = env('APP_NAME');\n",
    );

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", "<?php\n$name = env('APP_NAME');\n").await;

    // Line 1 (0-based), character 17 = inside "APP_NAME".
    let resp = s.definition("app.php", 1, 17).await;
    let out = render_locations(&resp, &s.uri(""));
    expect![".env:0:0-0:8"].assert_eq(&out);
}

/// A call wrapped across lines (common after formatter line-wrapping) must
/// still resolve — the string argument's own line has nothing but
/// whitespace before the quote, so the scan must look at the previous line
/// for the `env(` call.
#[tokio::test]
async fn env_call_goto_definition_resolves_when_call_wrapped_across_lines() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(
        workspace.path(),
        "APP_NAME=TestApp\nDB_HOST=127.0.0.1\n",
        "<?php\n$name = env(\n    'APP_NAME'\n);\n",
    );

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", "<?php\n$name = env(\n    'APP_NAME'\n);\n")
        .await;

    // Line 2 (0-based), character 8 = inside "APP_NAME" on its own line.
    let resp = s.definition("app.php", 2, 8).await;
    let out = render_locations(&resp, &s.uri(""));
    expect![".env:0:0-0:8"].assert_eq(&out);
}

#[tokio::test]
async fn env_call_falls_back_to_dot_env_example_when_key_only_there() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::write(workspace.path().join(".env"), "APP_NAME=TestApp\n").unwrap();
    std::fs::write(
        workspace.path().join(".env.example"),
        "APP_NAME=Example\nMAIL_MAILER=smtp\n",
    )
    .unwrap();
    let php = "<?php\n$mailer = env('MAIL_MAILER');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 25 = inside "MAIL_MAILER".
    let resp = s.definition("app.php", 1, 25).await;
    let out = render_locations(&resp, &s.uri(""));
    expect![".env.example:1:0-1:11"].assert_eq(&out);
}

#[tokio::test]
async fn env_call_completion_lists_env_keys_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(
        workspace.path(),
        "APP_NAME=TestApp\nAPP_ENV=local\nDB_HOST=127.0.0.1\n",
        "<?php\nenv('APP_\n",
    );

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", "<?php\nenv('APP_\n").await;

    // Line 1 (0-based), character 9 = right after "APP_".
    let resp = s.completion("app.php", 1, 9).await;
    let out = render_completion(&resp);
    expect![[r#"
        Constant    APP_ENV
        Constant    APP_NAME"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn env_call_not_resolved_outside_laravel_project() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    // No `artisan`, no Laravel composer.json — plain PHP project.
    let php = "<?php\n$name = env('APP_NAME');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.definition("app.php", 1, 17).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["<none>"].assert_eq(&out);
}

/// A key added to `.env` after startup resolves once the client reports the
/// change via `workspace/didChangeWatchedFiles`.
#[tokio::test]
async fn env_key_added_after_startup_resolves_after_watched_file_change() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    let php = "<?php\n$v = env('NEW_KEY');\n";
    write_minimal_laravel_project(workspace.path(), "APP_NAME=TestApp\n", php);

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;
    let resp = s.definition("app.php", 1, 13).await;
    expect!["<none>"].assert_eq(&render_locations(&resp, &s.uri("")));

    std::fs::write(
        workspace.path().join(".env"),
        "APP_NAME=TestApp\nNEW_KEY=1\n",
    )
    .unwrap();
    let env_uri = s.uri(".env");
    s.client()
        .notify(
            "workspace/didChangeWatchedFiles",
            serde_json::json!({ "changes": [{ "uri": env_uri, "type": 2 }] }),
        )
        .await;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let out = loop {
        let resp = s.definition("app.php", 1, 13).await;
        let out = render_locations(&resp, &s.uri(""));
        if out != "<none>" || std::time::Instant::now() > deadline {
            break out;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    expect![".env:1:0-1:7"].assert_eq(&out);
}
