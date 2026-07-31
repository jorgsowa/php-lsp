//! Protocol-wired tests for Laravel string-key quickfixes — offered when
//! `env('KEY')` or `__('...')`/`trans('...')` doesn't resolve to anything in
//! the workspace, against a synthetic minimal Laravel project.

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
}

// --- env('KEY') quickfix: "Add 'KEY' to .env" ---

#[tokio::test]
async fn env_missing_key_offers_add_to_dotenv_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    std::fs::write(workspace.path().join(".env"), "APP_NAME=Test\n").unwrap();
    let php = "<?php\nenv('DB_HOST');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 8 = inside "DB_HOST".
    let resp = s.code_action("app.php", 1, 8, 1, 8).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| a["title"].as_str() == Some("Add 'DB_HOST' to .env"))
        .cloned()
        .expect("expected the 'Add to .env' quickfix");
    assert_eq!(action["kind"].as_str(), Some("quickfix"));

    let out = canonicalize_workspace_edit(&action["edit"], &s.uri(""));
    expect![[r#"
        // .env
        0:0-0:0 → "DB_HOST=\n""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn env_existing_key_offers_no_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    std::fs::write(workspace.path().join(".env"), "APP_NAME=Test\n").unwrap();
    let php = "<?php\nenv('APP_NAME');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 8 = inside "APP_NAME".
    let resp = s.code_action("app.php", 1, 8, 1, 8).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str() == Some("Add 'APP_NAME' to .env")),
        "an already-declared key must not offer the quickfix, got: {actions:#?}"
    );
}

/// **LIMITATION**: creating `.env` from scratch is out of scope — the
/// quickfix is only offered when the file already exists.
#[tokio::test]
async fn env_missing_key_without_dotenv_file_offers_no_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\nenv('DB_HOST');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 8, 1, 8).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str().is_some_and(|t| t.contains(".env"))),
        "no .env file exists, so no quickfix should be offered, got: {actions:#?}"
    );
}

// --- __()/trans() quickfix: "Add "key" to <locale>.json" ---

#[tokio::test]
async fn translation_missing_literal_key_offers_json_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    std::fs::create_dir_all(workspace.path().join("lang")).unwrap();
    std::fs::write(
        workspace.path().join("lang").join("en.json"),
        r#"{"Hello": "Hello"}"#,
    )
    .unwrap();
    let php = "<?php\n__('Goodbye');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 6 = inside "Goodbye".
    let resp = s.code_action("app.php", 1, 6, 1, 6).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    let action = actions
        .iter()
        .find(|a| a["title"].as_str() == Some(r#"Add "Goodbye" to en.json"#))
        .cloned()
        .expect("expected the 'Add to en.json' quickfix");
    assert_eq!(action["kind"].as_str(), Some("quickfix"));

    let out = canonicalize_workspace_edit(&action["edit"], &s.uri(""));
    expect![[r#"
        // lang/en.json
        0:1-0:1 → "\"Goodbye\": \"\", ""#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn translation_existing_literal_key_offers_no_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    std::fs::create_dir_all(workspace.path().join("lang")).unwrap();
    std::fs::write(
        workspace.path().join("lang").join("en.json"),
        r#"{"Hello": "Hello"}"#,
    )
    .unwrap();
    let php = "<?php\n__('Hello');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 6 = inside "Hello".
    let resp = s.code_action("app.php", 1, 6, 1, 6).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str().is_some_and(|t| t.contains("en.json"))),
        "an already-declared key must not offer the quickfix, got: {actions:#?}"
    );
}

/// A dotted key with no matching `<group>.php` array file falls back to
/// Laravel's JSON-literal convention at runtime, so the quickfix must still
/// fire — same as any other literal key.
#[tokio::test]
async fn translation_missing_dotted_key_without_group_file_offers_json_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    std::fs::create_dir_all(workspace.path().join("lang")).unwrap();
    std::fs::write(workspace.path().join("lang").join("en.json"), "{}").unwrap();
    let php = "<?php\ntrans('messages.welcome');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 10 = inside "messages.welcome".
    let resp = s.code_action("app.php", 1, 10, 1, 10).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        actions
            .iter()
            .any(|a| a["title"].as_str() == Some(r#"Add "messages.welcome" to en.json"#)),
        "dotted key with no backing array file should fall back to the JSON quickfix, got: {actions:#?}"
    );
}

/// **LIMITATION**: when a `<group>.php` array file exists but is simply
/// missing this key, Laravel resolves via that file (returning the key
/// itself), never falling back to JSON — inserting into the PHP array would
/// risk producing invalid PHP, so no quickfix is offered for this case.
#[tokio::test]
async fn translation_missing_dotted_key_with_group_file_offers_no_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let en_dir = workspace.path().join("lang").join("en");
    std::fs::create_dir_all(&en_dir).unwrap();
    std::fs::write(
        en_dir.join("auth.php"),
        "<?php\nreturn ['failed' => 'Nope'];\n",
    )
    .unwrap();
    // A JSON file also exists, to prove its mere presence isn't enough to
    // trigger the quickfix once a matching group file is found.
    std::fs::write(workspace.path().join("lang").join("en.json"), "{}").unwrap();
    let php = "<?php\ntrans('auth.custom_message');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 10 = inside "auth.custom_message".
    let resp = s.code_action("app.php", 1, 10, 1, 10).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str().is_some_and(|t| t.contains("json"))),
        "a group file backing the dotted key must suppress the JSON quickfix, got: {actions:#?}"
    );
}

/// **LIMITATION**: no `lang/**/*.json` file exists anywhere in the workspace
/// to add the key to, so no quickfix is offered — matches the `.env`
/// no-target-file case above.
#[tokio::test]
async fn translation_missing_key_without_any_json_file_offers_no_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\n__('Goodbye');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 6, 1, 6).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str().is_some_and(|t| t.contains("json"))),
        "no json translation file exists, so no quickfix should be offered, got: {actions:#?}"
    );
}

// --- Documented Limitations: no quickfix infra for the remaining domains ---

/// **LIMITATION**: `config('a.b')` misses have no quickfix — inserting into
/// a possibly-nested PHP config array risks producing invalid PHP.
#[tokio::test]
async fn config_missing_key_offers_no_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    std::fs::create_dir_all(workspace.path().join("config")).unwrap();
    std::fs::write(
        workspace.path().join("config").join("app.php"),
        "<?php\nreturn ['name' => 'Test'];\n",
    )
    .unwrap();
    let php = "<?php\nconfig('app.missing');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 10 = inside "app.missing".
    let resp = s.code_action("app.php", 1, 10, 1, 10).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        actions.is_empty(),
        "config quickfixes are a known gap, got: {actions:#?}"
    );
}

/// **LIMITATION**: `view('a.b')` misses have no quickfix — scaffolding a new
/// Blade template requires creating a file, which this feature doesn't do.
#[tokio::test]
async fn view_missing_name_offers_no_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    std::fs::create_dir_all(workspace.path().join("resources").join("views")).unwrap();
    let php = "<?php\nview('missing.page');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 8 = inside "missing.page".
    let resp = s.code_action("app.php", 1, 8, 1, 8).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        actions.is_empty(),
        "view quickfixes are a known gap, got: {actions:#?}"
    );
}

/// `route('name')` misses offer a "Create route" quickfix
/// (`src/actions/route_scaffold_action.rs`) — see the dedicated test suite in
/// `feature_laravel_route_scaffold.rs` for the controller-scaffolding and
/// closure-fallback cases in depth; this test only confirms the quickfix
/// shows up at all in the general code-action listing.
#[tokio::test]
async fn route_missing_name_offers_create_route_quickfix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    std::fs::create_dir_all(workspace.path().join("routes")).unwrap();
    std::fs::write(
        workspace.path().join("routes").join("web.php"),
        "<?php\nRoute::get('/', Foo::class)->name('home');\n",
    )
    .unwrap();
    let php = "<?php\nroute('missing.route');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 8 = inside "missing.route".
    let resp = s.code_action("app.php", 1, 8, 1, 8).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        actions
            .iter()
            .any(|a| a["title"].as_str() == Some("Create route 'missing.route'")),
        "expected a 'Create route' quickfix, got: {actions:#?}"
    );
}

// --- Non-Laravel workspace: no quickfixes at all ---

#[tokio::test]
async fn env_missing_key_offers_no_quickfix_outside_laravel_project() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    // No `artisan`, no Laravel composer.json — plain PHP project.
    std::fs::write(workspace.path().join(".env"), "APP_NAME=Test\n").unwrap();
    let php = "<?php\nenv('DB_HOST');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.code_action("app.php", 1, 8, 1, 8).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str().is_some_and(|t| t.contains(".env"))),
        "non-Laravel workspaces must never offer Laravel quickfixes, got: {actions:#?}"
    );
}
