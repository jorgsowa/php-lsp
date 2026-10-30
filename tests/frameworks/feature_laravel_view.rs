//! Protocol-wired tests for Laravel `view('a.b.c')` go-to-definition and
//! completion, against a synthetic minimal Laravel project.

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    let views = root.join("resources").join("views");
    std::fs::create_dir_all(views.join("admin")).unwrap();
    std::fs::write(views.join("welcome.blade.php"), "<h1>Welcome</h1>\n").unwrap();
    std::fs::write(
        views.join("admin").join("dashboard.blade.php"),
        "<h1>Dashboard</h1>\n",
    )
    .unwrap();
}

#[tokio::test]
async fn view_call_goto_definition_resolves_top_level_view() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nreturn view('welcome');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 16 = inside "welcome".
    let resp = s.definition("app.php", 1, 16).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["resources/views/welcome.blade.php:0:0-0:0"].assert_eq(&out);
}

#[tokio::test]
async fn view_call_goto_definition_resolves_nested_view() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nreturn view('admin.dashboard');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 20 = inside "admin.dashboard".
    let resp = s.definition("app.php", 1, 20).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["resources/views/admin/dashboard.blade.php:0:0-0:0"].assert_eq(&out);
}

#[tokio::test]
async fn view_call_completion_lists_views_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nview('admin.\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 12 = right after "admin.".
    let resp = s.completion("app.php", 1, 12).await;
    let out = render_completion(&resp);
    expect!["File        admin.dashboard"].assert_eq(&out);
}

#[cfg(unix)]
#[tokio::test]
async fn views_and_config_behind_symlinks_are_indexed() {
    use std::os::unix::fs::symlink;
    let shared = tempfile::tempdir().expect("shared tempdir");
    std::fs::create_dir_all(shared.path().join("views/emails")).unwrap();
    std::fs::write(
        shared.path().join("views/emails/welcome.blade.php"),
        "<h1>Hi</h1>\n",
    )
    .unwrap();
    std::fs::create_dir_all(shared.path().join("config")).unwrap();
    std::fs::write(
        shared.path().join("config/mail.php"),
        "<?php\nreturn ['from' => 'a@b.c'];\n",
    )
    .unwrap();
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    symlink(
        shared.path().join("views/emails"),
        workspace.path().join("resources/views/emails"),
    )
    .unwrap();
    symlink(
        shared.path().join("config"),
        workspace.path().join("config"),
    )
    .unwrap();
    let php = "<?php\nreturn view('emails.welcome', ['f' => config('mail.from')]);\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.definition("app.php", 1, 20).await;
    expect!["resources/views/emails/welcome.blade.php:0:0-0:0"]
        .assert_eq(&render_locations(&resp, &s.uri("")));
    let resp = s.definition("app.php", 1, 52).await;
    expect!["config/mail.php:1:9-1:13"].assert_eq(&render_locations(&resp, &s.uri("")));
}
