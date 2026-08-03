//! Protocol-wired tests for Laravel `mix('path')` go-to-definition and
//! completion, against a synthetic minimal Laravel project.

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    let public = root.join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(
        public.join("mix-manifest.json"),
        r#"{
    "/css/app.css": "/css/app.css?id=abc123",
    "/js/app.js": "/js/app.js?id=def456"
}
"#,
    )
    .unwrap();
}

#[tokio::test]
async fn mix_call_goto_definition_resolves_manifest_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$href = mix('css/app.css');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 18 = inside "css/app.css".
    let resp = s.definition("app.php", 1, 18).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["public/mix-manifest.json:1:5-1:17"].assert_eq(&out);
}

#[tokio::test]
async fn mix_call_completion_lists_paths_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nmix('css/\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 9 = right after "css/".
    let resp = s.completion("app.php", 1, 9).await;
    let out = render_completion(&resp);
    expect!["File        css/app.css"].assert_eq(&out);
}

#[tokio::test]
async fn mix_call_not_resolved_outside_laravel_project() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    // No `artisan` marker — plain PHP project, even with a matching manifest.
    let public = workspace.path().join("public");
    std::fs::create_dir_all(&public).unwrap();
    std::fs::write(
        public.join("mix-manifest.json"),
        r#"{"/app.js": "/app.js?id=abc123"}"#,
    )
    .unwrap();
    let php = "<?php\n$href = mix('app.js');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.definition("app.php", 1, 18).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["<none>"].assert_eq(&out);
}
