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

/// Watched-file changes reload only the parts of the Laravel index they can
/// alter: a route save reloads routes, a Blade save reloads nothing, a new
/// Blade file reloads views. `.env` is rewritten on disk but never reported,
/// so it shows up only if something reloads the whole index.
#[tokio::test]
async fn watched_file_changes_reload_only_affected_laravel_parts() {
    const CREATED: u32 = 1;
    const CHANGED: u32 = 2;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::write(root.join(".env"), "APP_NAME=Test\n").unwrap();
    std::fs::create_dir_all(root.join("routes")).unwrap();
    std::fs::write(root.join("routes/web.php"), "<?php\n").unwrap();
    std::fs::create_dir_all(root.join("resources/views")).unwrap();
    std::fs::write(root.join("resources/views/home.blade.php"), "hi\n").unwrap();
    let php = "<?php\nenv('NEW_KEY');\nroute('fresh');\nview('later');\n";
    std::fs::write(root.join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(root).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;
    let def = async |s: &mut TestServer, line: u32, character: u32| {
        let resp = s.definition("app.php", line, character).await;
        render_locations(&resp, &s.uri(""))
    };
    let until_found = async |s: &mut TestServer, line: u32, character: u32| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let resp = s.definition("app.php", line, character).await;
            let out = render_locations(&resp, &s.uri(""));
            if out != "<none>" || std::time::Instant::now() > deadline {
                break out;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    };

    s.write_file(".env", "APP_NAME=Test\nNEW_KEY=1\n");
    s.write_file("resources/views/home.blade.php", "hello\n");
    let blade = s.uri("resources/views/home.blade.php");
    s.did_change_watched_files(vec![(blade, CHANGED)]).await;

    s.write_file(
        "routes/web.php",
        "<?php\nRoute::get('/', HomeController::class)->name('fresh');\n",
    );
    let routes = s.uri("routes/web.php");
    s.did_change_watched_files(vec![(routes, CHANGED)]).await;
    expect!["routes/web.php:1:46-1:51"].assert_eq(&until_found(&mut s, 2, 8).await);
    expect!["<none>"].assert_eq(&def(&mut s, 1, 6).await);

    s.write_file("resources/views/later.blade.php", "later\n");
    let later = s.uri("resources/views/later.blade.php");
    s.did_change_watched_files(vec![(later, CREATED)]).await;
    expect!["resources/views/later.blade.php:0:0-0:0"].assert_eq(&until_found(&mut s, 3, 7).await);
    expect!["<none>"].assert_eq(&def(&mut s, 1, 6).await);
}
