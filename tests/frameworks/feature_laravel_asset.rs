//! Protocol-wired tests for Laravel `asset('path')` go-to-definition and
//! completion, against a synthetic minimal Laravel project.

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    let public = root.join("public");
    std::fs::create_dir_all(public.join("css")).unwrap();
    std::fs::write(public.join("css").join("app.css"), "body {}\n").unwrap();
    std::fs::write(public.join("favicon.ico"), "x").unwrap();
}

#[tokio::test]
async fn asset_call_goto_definition_resolves_nested_file() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$href = asset('css/app.css');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 20 = inside "css/app.css".
    let resp = s.definition("app.php", 1, 20).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["public/css/app.css:0:0-0:0"].assert_eq(&out);
}

#[tokio::test]
async fn asset_call_completion_lists_paths_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nasset('css/\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 11 = right after "css/".
    let resp = s.completion("app.php", 1, 11).await;
    let out = render_completion(&resp);
    expect!["File        css/app.css"].assert_eq(&out);
}

#[tokio::test]
async fn asset_call_not_resolved_outside_laravel_project() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    // No `artisan` marker — plain PHP project, even with a matching file on disk.
    std::fs::create_dir_all(workspace.path().join("public")).unwrap();
    std::fs::write(workspace.path().join("public").join("app.css"), "x").unwrap();
    let php = "<?php\n$href = asset('app.css');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.definition("app.php", 1, 20).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["<none>"].assert_eq(&out);
}
