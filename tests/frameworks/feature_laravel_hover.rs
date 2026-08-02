//! Protocol-wired tests for hover over Laravel string-key calls (`env`,
//! `config`, `view`, `trans`/`__`, `route`, `asset`, `->middleware(...)`),
//! against a synthetic minimal Laravel project covering every domain.

use super::*;

use expect_test::expect;

fn write_full_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::write(root.join(".env"), "APP_NAME=Test\n").unwrap();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(
        root.join("config").join("app.php"),
        "<?php\nreturn [\n    'name' => 'Laravel',\n];\n",
    )
    .unwrap();
    let views = root.join("resources").join("views");
    std::fs::create_dir_all(&views).unwrap();
    std::fs::write(views.join("welcome.blade.php"), "<h1>Welcome</h1>\n").unwrap();
    let en = root.join("lang").join("en");
    std::fs::create_dir_all(&en).unwrap();
    std::fs::write(
        en.join("auth.php"),
        "<?php\nreturn [\n    'failed' => 'These credentials do not match.',\n];\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("routes")).unwrap();
    std::fs::write(
        root.join("routes").join("web.php"),
        "<?php\nRoute::get('/', HomeController::class)->name('home');\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("public").join("css")).unwrap();
    std::fs::write(root.join("public").join("css").join("app.css"), "body {}\n").unwrap();
    std::fs::create_dir_all(root.join("bootstrap")).unwrap();
    std::fs::write(
        root.join("bootstrap").join("app.php"),
        "<?php\nreturn Application::configure()\n    ->withMiddleware(function (Middleware $middleware) {\n        $middleware->alias([\n            'auth' => \\App\\Http\\Middleware\\Authenticate::class,\n        ]);\n    })->create();\n",
    )
    .unwrap();
}

#[tokio::test]
async fn env_call_hover_shows_dot_env_value() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\n$name = env('APP_NAME');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 17 = inside "APP_NAME".
    let resp = s.hover("app.php", 1, 17).await;
    let out = render_hover(&resp);
    expect![[r#"
        **env('APP_NAME')**

        ```properties
        APP_NAME=Test
        ```

        `.env`"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn config_call_hover_shows_defining_line() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\n$name = config('app.name');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 21 = inside "app.name".
    let resp = s.hover("app.php", 1, 21).await;
    let out = render_hover(&resp);
    expect![[r#"
        **config('app.name')**

        ```php
        'name' => 'Laravel',
        ```

        `config/app.php`"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn view_call_hover_shows_resolved_path_without_snippet() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\nreturn view('welcome');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 16 = inside "welcome".
    let resp = s.hover("app.php", 1, 16).await;
    let out = render_hover(&resp);
    expect![[r#"
        **view('welcome')**

        `resources/views/welcome.blade.php`"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn trans_call_hover_shows_translation_value() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\necho __('auth.failed');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 10 = inside "auth.failed".
    let resp = s.hover("app.php", 1, 10).await;
    let out = render_hover(&resp);
    expect![[r#"
        **trans('auth.failed')**

        ```php
        'failed' => 'These credentials do not match.',
        ```

        `lang/en/auth.php`"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn route_call_hover_shows_registration_line() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\n$url = route('home');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 16 = inside "home".
    let resp = s.hover("app.php", 1, 16).await;
    let out = render_hover(&resp);
    expect![[r#"
        **route('home')**

        ```php
        Route::get('/', HomeController::class)->name('home');
        ```

        `routes/web.php`"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn asset_call_hover_shows_resolved_path_without_snippet() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\n$href = asset('css/app.css');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 20 = inside "css/app.css".
    let resp = s.hover("app.php", 1, 20).await;
    let out = render_hover(&resp);
    expect![[r#"
        **asset('css/app.css')**

        `public/css/app.css`"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn middleware_call_hover_shows_alias_registration() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_full_laravel_project(workspace.path());
    let php = "<?php\nRoute::get('/', HomeController::class)->middleware('auth');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 54 = inside "auth".
    let resp = s.hover("app.php", 1, 54).await;
    let out = render_hover(&resp);
    expect![[r#"
        **middleware('auth')**

        ```php
        'auth' => \App\Http\Middleware\Authenticate::class,
        ```

        `bootstrap/app.php`"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn env_call_hover_none_outside_laravel_project() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    // No `artisan` marker — plain PHP project.
    let php = "<?php\n$name = env('APP_NAME');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();
    std::fs::write(workspace.path().join(".env"), "APP_NAME=Test\n").unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.hover("app.php", 1, 17).await;
    let out = render_hover(&resp);
    expect!["<no hover>"].assert_eq(&out);
}
