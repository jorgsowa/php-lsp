//! Warnings for Laravel helper calls whose string key resolves to nothing.

use super::*;

use serde_json::{Value, json};

const CODES: &[&str] = &[
    "UndefinedEnvKey",
    "UndefinedConfigKey",
    "UndefinedRoute",
    "UndefinedView",
    "UndefinedTranslationKey",
];

fn write(root: &std::path::Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn laravel_project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(root, "artisan", "#!/usr/bin/env php\n");
    write(root, ".env", "APP_NAME=Laravel\n");
    write(
        root,
        "config/app.php",
        "<?php\nreturn ['name' => env('APP_NAME', 'Laravel')];\n",
    );
    write(
        root,
        "routes/web.php",
        "<?php\nRoute::view('/', 'welcome')->name('home');\n",
    );
    write(
        root,
        "app/Providers/FolioServiceProvider.php",
        "<?php\nclass FolioServiceProvider {\n    public function boot(): void { Folio::path(resource_path('views/folio')); }\n}\n",
    );
    write(
        root,
        "resources/views/folio/explore.blade.php",
        "<?php\nuse function Laravel\\Folio\\name;\nname('explore');\n?>\n<div></div>\n",
    );
    write(root, "resources/views/welcome.blade.php", "<h1>Hi</h1>\n");
    write(
        root,
        "lang/en/boards.php",
        "<?php\nreturn ['explore' => 'Explore', 'visibility' => ['public' => 'Public']];\n",
    );
    tmp
}

const SURFACE: &str = "<?php
function surface(): void {
    env('APP_NAME');
    env('');
    env('GATE_MISSING_KEY');
    env('OPTIONAL_KEY', 'fallback');
    route('home');
    route('explore');
    route('gate.missing');
    view('welcome');
    view('missing.page');
    view('mail::message');
    config('app.name');
    config('gate.missing');
    config('app.missing');
    __('boards.explore');
    __('boards.visibility.public');
    __('boards.missing');
    __('Welcome back.');
}
";

/// `line: code 'key'` for every missing-key diagnostic in `diagnostics`.
fn render(diagnostics: &Value) -> Vec<String> {
    let mut out: Vec<String> = diagnostics
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["code"].as_str().is_some_and(|c| CODES.contains(&c)))
        .map(|d| {
            format!(
                "{}: {} ({}:{}-{})",
                d["range"]["start"]["line"],
                d["code"].as_str().unwrap(),
                d["severity"],
                d["range"]["start"]["character"],
                d["range"]["end"]["character"],
            )
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn push_diagnostics_flag_undefined_helper_keys() {
    let tmp = laravel_project();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let published = s.open("app/Support/LspSurface.php", SURFACE).await;
    assert_eq!(
        render(&published["params"]["diagnostics"]),
        [
            "10: UndefinedView (2:10-22)",
            "13: UndefinedConfigKey (2:12-24)",
            "14: UndefinedConfigKey (2:12-23)",
            "17: UndefinedTranslationKey (2:8-22)",
            "4: UndefinedEnvKey (2:9-25)",
            "8: UndefinedRoute (2:11-23)",
        ],
    );
    let messages: Vec<&str> = published["params"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["message"].as_str())
        .collect();
    for key in [
        "GATE_MISSING_KEY",
        "gate.missing",
        "missing.page",
        "boards.missing",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(key)),
            "no message names {key}: {messages:?}"
        );
    }
}

#[tokio::test]
async fn pull_diagnostics_flag_undefined_helper_keys() {
    let tmp = laravel_project();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app/Support/LspSurface.php", SURFACE).await;

    let resp = s.pull_diagnostics("app/Support/LspSurface.php").await;
    assert_eq!(render(&resp["result"]["items"]).len(), 6, "{resp}");
}

#[tokio::test]
async fn disabled_codes_are_not_reported() {
    let tmp = laravel_project();
    let mut s = TestServer::with_root_and_options(
        tmp.path(),
        json!({ "diagnostics": { "disabled": ["UndefinedRoute", "UndefinedEnvKey"] } }),
    )
    .await;
    s.wait_for_index_ready().await;

    let published = s.open("app/Support/LspSurface.php", SURFACE).await;
    let rendered = render(&published["params"]["diagnostics"]);
    assert_eq!(rendered.len(), 4, "{rendered:?}");
    assert!(
        rendered
            .iter()
            .all(|d| !d.contains("UndefinedRoute") && !d.contains("UndefinedEnvKey")),
        "{rendered:?}"
    );
}

#[tokio::test]
async fn bare_env_reads_in_config_files_are_not_reported() {
    let tmp = laravel_project();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let published = s
        .open(
            "config/services.php",
            "<?php\nreturn ['key' => env('POSTMARK_API_KEY'), 'view' => view('missing.page')];\n",
        )
        .await;
    assert_eq!(
        render(&published["params"]["diagnostics"]),
        ["1: UndefinedView (2:58-70)"]
    );
}

#[tokio::test]
async fn non_laravel_workspace_reports_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let published = s.open("surface.php", SURFACE).await;
    assert!(render(&published["params"]["diagnostics"]).is_empty());
}
