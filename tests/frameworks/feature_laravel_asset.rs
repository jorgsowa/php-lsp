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

/// The client is asked to report files created or deleted under `public/`,
/// which is what reloads the asset index.
#[tokio::test]
async fn public_dir_creations_and_deletions_are_watched() {
    let (mut s, _) = TestServer::new_with_client_capabilities(serde_json::json!({})).await;
    let (_, params) = s
        .client()
        .expect_server_request("client/registerCapability")
        .await;
    let watchers = params["registrations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "php-lsp-file-watcher")
        .map(|r| r["registerOptions"]["watchers"].clone())
        .expect("file watcher registration");
    let public = watchers
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["globPattern"] == "**/public/**")
        .expect("watcher for public/");
    // WatchKind::Create | WatchKind::Delete
    assert_eq!(public["kind"], 5);
}

#[tokio::test]
async fn asset_created_after_startup_resolves_after_watched_file_change() {
    const CREATED: u32 = 1;
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$href = asset('js/new.js');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;
    let resp = s.definition("app.php", 1, 20).await;
    expect!["<none>"].assert_eq(&render_locations(&resp, &s.uri("")));

    s.write_file("public/js/new.js", "x\n");
    let created = s.uri("public/js/new.js");
    s.did_change_watched_files(vec![(created, CREATED)]).await;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let out = loop {
        let resp = s.definition("app.php", 1, 20).await;
        let out = render_locations(&resp, &s.uri(""));
        if out != "<none>" || std::time::Instant::now() > deadline {
            break out;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    expect!["public/js/new.js:0:0-0:0"].assert_eq(&out);
}
