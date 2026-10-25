//! Protocol-wired tests for Laravel `__('a.b')` / `trans('a.b')`
//! go-to-definition and completion, against a synthetic minimal Laravel
//! project.

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
    let en = root.join("lang").join("en");
    std::fs::create_dir_all(&en).unwrap();
    std::fs::write(
        en.join("auth.php"),
        "<?php\nreturn [\n    'failed' => 'These credentials do not match.',\n];\n",
    )
    .unwrap();
}

#[tokio::test]
async fn dunder_call_goto_definition_resolves_translation_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\necho __('auth.failed');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 10 = inside "auth.failed".
    let resp = s.definition("app.php", 1, 10).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["lang/en/auth.php:2:5-2:11"].assert_eq(&out);
}

#[tokio::test]
async fn trans_call_goto_definition_resolves_translation_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\necho trans('auth.failed');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 14 = inside "auth.failed".
    let resp = s.definition("app.php", 1, 14).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["lang/en/auth.php:2:5-2:11"].assert_eq(&out);
}

#[tokio::test]
async fn trans_call_completion_lists_translation_keys_by_prefix() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    write_minimal_laravel_project(workspace.path());
    let php = "<?php\ntrans('auth.\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 12 = right after "auth.".
    let resp = s.completion("app.php", 1, 12).await;
    let out = render_completion(&resp);
    expect!["Text        auth.failed"].assert_eq(&out);
}

/// JSON translation keys may be written with escapes; a key whose text also
/// appears earlier as a value must still resolve to the key.
#[tokio::test]
async fn dunder_call_goto_definition_resolves_escaped_json_key() {
    let workspace = tempfile::tempdir().expect("workspace tempdir");
    std::fs::write(workspace.path().join("artisan"), "#!/usr/bin/env php\n").unwrap();
    let lang = workspace.path().join("lang");
    std::fs::create_dir_all(&lang).unwrap();
    std::fs::write(
        lang.join("fr.json"),
        "{\n    \"Coffee\": \"Caf\\u00e9\",\n    \"Caf\\u00e9\": \"Caf\\u00e9\"\n}\n",
    )
    .unwrap();
    let php = "<?php\necho __('Café');\n";
    std::fs::write(workspace.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(workspace.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.definition("app.php", 1, 10).await;
    let out = render_locations(&resp, &s.uri(""));
    expect!["lang/fr.json:2:5-2:14"].assert_eq(&out);
}
