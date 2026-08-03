//! Protocol-wired tests for Laravel `vite('path')` / `Vite::asset('path')`
//! go-to-definition and completion, against a synthetic minimal Laravel
//! project.

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    let build = root.join("public").join("build");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(
        build.join("manifest.json"),
        r#"{
    "resources/css/app.css": {"file": "assets/app-4ed993c7.css"},
    "resources/js/app.js": {"file": "assets/app-3f5d7f7a.js"}
}
"#,
    )
    .unwrap();
}

#[tokio::test]
async fn vite_call_goto_definition_resolves_manifest_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$tags = vite('resources/js/app.js');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 20 = inside "resources/js/app.js".
    let resp = s.definition("app.php", 1, 20).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["public/build/manifest.json:2:5-2:24"].assert_eq(&out);
}

#[tokio::test]
async fn vite_asset_static_call_goto_definition_resolves_manifest_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$href = Vite::asset('resources/css/app.css');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 28 = inside "resources/css/app.css".
    let resp = s.definition("app.php", 1, 28).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["public/build/manifest.json:1:5-1:26"].assert_eq(&out);
}

#[tokio::test]
async fn vite_call_completion_lists_paths_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nvite('resources/j\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 17 = right after "resources/j".
    let resp = s.completion("app.php", 1, 17).await;
    let out = render_completion(&resp);
    expect!["File        resources/js/app.js"].assert_eq(&out);
}

#[tokio::test]
async fn vite_asset_static_call_completion_lists_paths_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\necho Vite::asset('resources/c\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 29 = right after "resources/c".
    let resp = s.completion("app.php", 1, 29).await;
    let out = render_completion(&resp);
    expect!["File        resources/css/app.css"].assert_eq(&out);
}

#[tokio::test]
async fn vite_call_not_resolved_outside_laravel_project() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    // No `artisan` marker — plain PHP project, even with a matching manifest.
    let build = workspace.path().join("public").join("build");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(
        build.join("manifest.json"),
        r#"{"resources/js/app.js": {"file": "assets/app.js"}}"#,
    )
    .unwrap();
    let php = "<?php\n$tags = vite('resources/js/app.js');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.definition("app.php", 1, 20).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["<none>"].assert_eq(&out);
}
