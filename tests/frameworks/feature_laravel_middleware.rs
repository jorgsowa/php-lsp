//! Protocol-wired tests for Laravel `->middleware('alias')` /
//! `Route::middleware([...])` go-to-definition and completion, against a
//! synthetic minimal Laravel project.

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::create_dir_all(root.join("bootstrap")).unwrap();
    std::fs::write(
        root.join("bootstrap").join("app.php"),
        "<?php\nreturn Application::configure()\n    ->withMiddleware(function (Middleware $middleware) {\n        $middleware->alias([\n            'auth' => \\App\\Http\\Middleware\\Authenticate::class,\n            'auth.basic' => \\App\\Http\\Middleware\\AuthenticateWithBasicAuth::class,\n            'throttle' => \\App\\Http\\Middleware\\ThrottleRequests::class,\n        ]);\n    })->create();\n",
    )
    .unwrap();
}

#[tokio::test]
async fn middleware_call_goto_definition_resolves_single_string_form() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nRoute::get('/', HomeController::class)->middleware('auth');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 54 = inside "auth".
    let resp = s.definition("app.php", 1, 54).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["bootstrap/app.php:4:13-4:17"].assert_eq(&out);
}

#[tokio::test]
async fn middleware_call_goto_definition_resolves_array_element() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nRoute::middleware(['auth', 'auth.basic'])->group(function () {});\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 30 = inside "auth.basic".
    let resp = s.definition("app.php", 1, 30).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["bootstrap/app.php:5:13-5:23"].assert_eq(&out);
}

#[tokio::test]
async fn middleware_call_goto_definition_strips_parameters() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nRoute::get('/', HomeController::class)->middleware('throttle:60,1');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 54 = inside "throttle" (before the `:60,1` suffix).
    let resp = s.definition("app.php", 1, 54).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["bootstrap/app.php:6:13-6:21"].assert_eq(&out);
}

#[tokio::test]
async fn middleware_call_completion_lists_aliases_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nRoute::middleware('au\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 21 = right after "au".
    let resp = s.completion("app.php", 1, 21).await;
    let out = render_completion(&resp);
    expect![[r#"
        Constant    auth
        Constant    auth.basic"#]]
    .assert_eq(&out);
}
