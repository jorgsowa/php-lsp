//! Protocol-wired tests for validation rule-name completion inside
//! `->validate([...])`/`Validator::make([...])`/a `rules()` method's
//! `return [...]` (`src/laravel/validation_rules.rs`).

use super::*;

use expect_test::expect;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
}

#[tokio::test]
async fn completion_suggests_rule_names_in_pipe_form_first_rule() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let php = "<?php\nclass Controller {\n    public function store($request) {\n        $request->validate(['email' => 'requ\n    }\n}\n";
    std::fs::write(tmp.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 3 (0-based), character 44 = right after "requ".
    let resp = s.completion("app.php", 3, 44).await;
    let out = render_completion(&resp);
    assert!(
        out.contains("Keyword     required"),
        "expected 'required' rule completion, got:\n{out}"
    );
}

#[tokio::test]
async fn completion_suggests_rule_names_after_pipe() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let php = "<?php\nclass Controller {\n    public function store($request) {\n        $request->validate(['email' => 'required|em\n    }\n}\n";
    std::fs::write(tmp.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 3 (0-based), character 51 = right after "em" (following "required|").
    let resp = s.completion("app.php", 3, 51).await;
    let out = render_completion(&resp);
    expect!["Keyword     email"].assert_eq(&out);
}

#[tokio::test]
async fn completion_suggests_rule_names_in_array_form() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let php = "<?php\nclass Controller {\n    public function store($request) {\n        $request->validate(['email' => ['required', 'em\n    }\n}\n";
    std::fs::write(tmp.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 3 (0-based), character 55 = right after "em" in the array form.
    let resp = s.completion("app.php", 3, 55).await;
    let out = render_completion(&resp);
    expect!["Keyword     email"].assert_eq(&out);
}

#[tokio::test]
async fn completion_suggests_rule_names_in_validator_make_second_argument() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let php = "<?php\nValidator::make($data, ['email' => 'requ\n";
    std::fs::write(tmp.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 1 (0-based), character 40 = right after "requ".
    let resp = s.completion("app.php", 1, 40).await;
    let out = render_completion(&resp);
    assert!(
        out.contains("Keyword     required"),
        "expected 'required' rule completion from Validator::make(), got:\n{out}"
    );
}

#[tokio::test]
async fn completion_suggests_rule_names_in_rules_method_return() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let php = "<?php\nclass StoreUserRequest {\n    public function rules() {\n        return ['email' => 'requ\n    }\n}\n";
    std::fs::write(tmp.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 3 (0-based), character 32 = right after "requ".
    let resp = s.completion("app.php", 3, 32).await;
    let out = render_completion(&resp);
    assert!(
        out.contains("Keyword     required"),
        "expected 'required' rule completion from a rules() method return, got:\n{out}"
    );
}

#[tokio::test]
async fn completion_does_not_offer_rule_names_while_typing_the_field_key() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    // No `=>` yet — still typing the field name, not a rule value.
    let php = "<?php\nclass Controller {\n    public function store($request) {\n        $request->validate(['requ\n    }\n}\n";
    std::fs::write(tmp.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 3 (0-based), character 33 = right after "requ".
    let resp = s.completion("app.php", 3, 33).await;
    let out = render_completion(&resp);
    assert!(
        !out.contains("Keyword     required"),
        "must not offer rule completions while typing the field key, got:\n{out}"
    );
}

#[tokio::test]
async fn completion_ignores_unrelated_array_literal() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let php = "<?php\nclass Controller {\n    public function store() {\n        $x = ['email' => 'requ\n    }\n}\n";
    std::fs::write(tmp.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    // Line 3 (0-based), character 30 = right after "requ".
    let resp = s.completion("app.php", 3, 30).await;
    let out = render_completion(&resp);
    assert!(
        !out.contains("Keyword     required"),
        "a plain array unrelated to validate()/rules() must not get rule completions, got:\n{out}"
    );
}

#[tokio::test]
async fn not_offered_in_non_laravel_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    // Deliberately no `artisan` — not a Laravel workspace.
    let php = "<?php\nclass Controller {\n    public function store($request) {\n        $request->validate(['email' => 'requ\n    }\n}\n";
    std::fs::write(tmp.path().join("app.php"), php).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;
    s.open("app.php", php).await;

    let resp = s.completion("app.php", 3, 44).await;
    let out = render_completion(&resp);
    assert!(
        !out.contains("Keyword     required"),
        "non-Laravel workspaces must never offer validation rule completions, got:\n{out}"
    );
}
