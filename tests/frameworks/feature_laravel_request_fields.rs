//! Regression tests for request-field completion and the "Generate
//! validation rules from $request usages" quickfix
//! (`src/laravel/request_fields.rs`, `src/actions/generate_validation_rules_action.rs`).
//!
//! Both are naming-convention heuristics (a variable literally named
//! `$request`, or `$this` inside a `rules()`-bearing class) rather than
//! type-checked — see the module doc on `request_fields.rs` for why.

use super::*;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
}

const VALIDATE_TITLE: &str = "Generate validation rules from $request usages";

#[tokio::test]
async fn completion_suggests_fields_harvested_from_other_calls() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let php = "<?php\nclass Controller {\n    public function store($request) {\n        $request->input('email');\n        $request->get('name');\n        $request->input('');\n    }\n}\n";
    s.open("controller.php", php).await;

    // Line 5 (0-based), character 25 = right after the opening quote of the
    // empty string in `$request->input('');`.
    let resp = s.completion("controller.php", 5, 25).await;
    let out = render_completion(&resp);
    assert!(
        out.contains("email") && out.contains("name"),
        "expected both harvested fields, got:\n{out}"
    );
}

#[tokio::test]
async fn completion_ignores_variable_not_named_request() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let php = "<?php\nfunction f($other) {\n    $other->input('em');\n}\n";
    s.open("app.php", php).await;

    // Line 2, character 19 = right after "em" inside the string.
    let resp = s.completion("app.php", 2, 19).await;
    let out = render_completion(&resp);
    assert!(
        !out.contains("email"),
        "a variable not named $request must not get request-field completions, got:\n{out}"
    );
}

#[tokio::test]
async fn generates_rules_from_request_usages_in_same_method() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let out = s
        .check_code_action_apply(
            r#"<?php
class Controller {
    public function store($request) {
        $request->input('email');
        $request->input('name');
        $request->validate([$0]);
    }
}
"#,
            VALIDATE_TITLE,
        )
        .await;

    assert!(
        out.contains("'email' => 'required'") && out.contains("'name' => 'required'"),
        "expected both harvested fields as rule stubs, got:\n{out}"
    );
}

#[tokio::test]
async fn generates_rules_for_form_request_from_other_class_methods() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let out = s
        .check_code_action_apply(
            r#"<?php
class StoreUserRequest {
    public function prepareForValidation() {
        $this->input('email');
    }
    public function rules() {
        return [$0];
    }
}
"#,
            VALIDATE_TITLE,
        )
        .await;

    assert!(
        out.contains("'email' => 'required'"),
        "rules() has nothing of its own to harvest from — must pull from other methods in the same class, got:\n{out}"
    );
}

#[tokio::test]
async fn does_not_duplicate_an_already_present_field() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let php = "<?php\nclass Controller {\n    public function store($request) {\n        $request->input('email');\n        $request->validate(['email' => 'required']);\n    }\n}\n";
    s.open("controller.php", php).await;

    // Line 4 (0-based), somewhere inside the array literal.
    let resp = s.code_action("controller.php", 4, 30, 4, 30).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str() == Some(VALIDATE_TITLE)),
        "every harvested field is already present — no quickfix should be offered"
    );
}

#[tokio::test]
async fn not_offered_in_non_laravel_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    // Deliberately no `artisan` — not a Laravel workspace.
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    let php = "<?php\nclass Controller {\n    public function store($request) {\n        $request->input('email');\n        $request->validate([]);\n    }\n}\n";
    s.open("controller.php", php).await;

    let resp = s.code_action("controller.php", 4, 28, 4, 28).await;
    let actions = resp["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !actions
            .iter()
            .any(|a| a["title"].as_str() == Some(VALIDATE_TITLE)),
        "non-Laravel workspaces must never offer this quickfix"
    );

    let resp = s.completion("controller.php", 3, 25).await;
    let out = render_completion(&resp);
    assert!(
        !out.contains("email"),
        "non-Laravel workspaces must never offer request-field completion, got:\n{out}"
    );
}
