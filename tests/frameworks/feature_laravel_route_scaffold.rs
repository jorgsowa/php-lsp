//! Regression tests for the "Create route" quickfix
//! (`src/actions/route_scaffold_action.rs`), offered on a `route('...')`
//! call whose name doesn't resolve in `RouteIndex`.
//!
//! Uses `TestServer::with_root` + a real on-disk `artisan`/`routes/web.php`
//! (Laravel detection and the routes-file edit both read the filesystem),
//! with controller classes opened over the wire rather than written to disk
//! — `docs_for_scan_mentioning` (the same mechanism `implement_action`'s
//! cross-file quickfix uses) covers open files regardless of on-disk
//! presence.

use super::*;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    std::fs::create_dir_all(root.join("routes")).unwrap();
    std::fs::write(
        root.join("routes").join("web.php"),
        "<?php\n\nRoute::get('/', HomeController::class)->name('home');\n",
    )
    .unwrap();
}

#[tokio::test]
async fn falls_back_to_closure_when_no_controller_exists() {
    let workspace = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$url = route('posts.show');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 15, 1, 15).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| a["title"].as_str() == Some("Create route 'posts.show'"))
        .expect("expected a 'Create route' quickfix");

    let out = canonicalize_workspace_edit(&action["edit"], &s.uri(""));
    assert!(
        out.contains("routes/web.php"),
        "edit should touch routes/web.php, got: {out}"
    );
    assert!(
        out.contains("function () {") && out.contains("TODO: implement 'posts.show'"),
        "with no PostsController anywhere in the workspace, must fall back to a closure, got: {out}"
    );
}

#[tokio::test]
async fn appends_method_to_existing_controller() {
    let workspace = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$url = route('posts.show');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open(
        "app/Http/Controllers/PostsController.php",
        "<?php\nnamespace App\\Http\\Controllers;\nclass PostsController {\n    public function index() {}\n}\n",
    )
    .await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 15, 1, 15).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| a["title"].as_str() == Some("Create route 'posts.show'"))
        .expect("expected a 'Create route' quickfix");

    let out = canonicalize_workspace_edit(&action["edit"], &s.uri(""));
    assert!(
        out.contains("PostsController::class"),
        "route line should reference the existing controller's FQN, got: {out}"
    );
    assert!(
        out.contains("PostsController.php") && out.contains("public function show()"),
        "a `show` method stub should be appended to the existing controller, got: {out}"
    );
}

#[tokio::test]
async fn does_not_duplicate_an_existing_method() {
    let workspace = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$url = route('posts.show');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open(
        "app/Http/Controllers/PostsController.php",
        "<?php\nnamespace App\\Http\\Controllers;\nclass PostsController {\n    public function show() {}\n}\n",
    )
    .await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 15, 1, 15).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| a["title"].as_str() == Some("Create route 'posts.show'"))
        .expect("expected a 'Create route' quickfix");

    let out = canonicalize_workspace_edit(&action["edit"], &s.uri(""));
    assert!(
        !out.contains("PostsController.php"),
        "an already-existing `show` method must not get a duplicate stub, got: {out}"
    );
    assert!(
        out.contains("PostsController::class"),
        "the route line itself should still be added, got: {out}"
    );
}

#[tokio::test]
async fn falls_back_to_closure_for_name_without_a_dot() {
    let workspace = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$url = route('dashboard');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 15, 1, 15).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| a["title"].as_str() == Some("Create route 'dashboard'"))
        .expect("expected a 'Create route' quickfix");

    let out = canonicalize_workspace_edit(&action["edit"], &s.uri(""));
    assert!(
        out.contains("/dashboard") && out.contains("function () {"),
        "a name with no `.` has no resource/action to derive a controller from, got: {out}"
    );
}

#[tokio::test]
async fn not_offered_for_an_already_resolvable_route() {
    let workspace = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n$url = route('home');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 15, 1, 15).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str() == Some("Create route 'home'")),
        "'home' already resolves via RouteIndex — no quickfix should be offered"
    );
}

#[tokio::test]
async fn not_offered_in_non_laravel_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    // Deliberately no `artisan` / `routes/web.php` — not a Laravel workspace.
    let php = "<?php\n$url = route('posts.show');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 15, 1, 15).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str() == Some("Create route 'posts.show'")),
        "non-Laravel workspaces must never offer this quickfix"
    );
}
