//! Regression tests for the unguarded-Eloquent-mass-assignment diagnostic
//! (`src/laravel/eloquent_guard.rs`). Fires only for classes that transitively
//! extend `Illuminate\Database\Eloquent\Model` and explicitly declare
//! `protected $guarded = [];` — never for models that simply omit
//! `$fillable`/`$guarded` (Laravel's own default there is fully guarded, so
//! that case must NOT be flagged).
//!
//! Uses a real on-disk `artisan` marker (Laravel-project detection reads the
//! filesystem, not open buffers) plus in-memory fixture files opened over the
//! wire for the classes under test — cross-file class-hierarchy resolution
//! works for opened-but-not-on-disk files (see the interface-implementation
//! tests in `feature_code_action_implement_interface.rs`), so no real vendor
//! tree is needed. Asserts by filtering `textDocument/diagnostic` pull
//! results down to this diagnostic's own `code`, rather than the caret-DSL's
//! closed-world match, since mir's own semantic analysis may also produce
//! diagnostics against these deliberately-incomplete stub classes (e.g. an
//! undefined `create`/`fill` method) that aren't the point of this test.

use super::*;

use serde_json::Value;

fn write_minimal_laravel_project(root: &std::path::Path) {
    std::fs::write(root.join("artisan"), "#!/usr/bin/env php\n").unwrap();
}

fn unguarded_messages(resp: &Value) -> Vec<String> {
    let mut out: Vec<String> = resp["result"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["code"].as_str() == Some("UnguardedMassAssignment"))
        .map(|d| d["message"].as_str().unwrap_or("").to_string())
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn flags_create_call_on_explicitly_unguarded_model() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "Models/Post.php",
        "<?php\nclass Post extends \\Illuminate\\Database\\Eloquent\\Model {\n    protected $guarded = [];\n}\n",
    )
    .await;
    s.open(
        "app.php",
        "<?php\nfunction handle(): void {\n    Post::create(['title' => 'hi']);\n}\n",
    )
    .await;

    let resp = s.pull_diagnostics("app.php").await;
    let messages = unguarded_messages(&resp);
    assert_eq!(
        messages.len(),
        1,
        "expected exactly one diagnostic, got: {messages:?}"
    );
    assert!(
        messages[0].contains("Post"),
        "message should name the model: {messages:?}"
    );
}

#[tokio::test]
async fn does_not_flag_model_with_nonempty_fillable() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "Models/Post.php",
        "<?php\nclass Post extends \\Illuminate\\Database\\Eloquent\\Model {\n    protected $fillable = ['title'];\n}\n",
    )
    .await;
    s.open(
        "app.php",
        "<?php\nfunction handle(): void {\n    Post::create(['title' => 'hi']);\n}\n",
    )
    .await;

    let resp = s.pull_diagnostics("app.php").await;
    assert!(
        unguarded_messages(&resp).is_empty(),
        "a model with a non-empty $fillable must not be flagged"
    );
}

#[tokio::test]
async fn does_not_flag_model_with_neither_fillable_nor_guarded() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "Models/Post.php",
        "<?php\nclass Post extends \\Illuminate\\Database\\Eloquent\\Model {\n}\n",
    )
    .await;
    s.open(
        "app.php",
        "<?php\nfunction handle(): void {\n    Post::create(['title' => 'hi']);\n}\n",
    )
    .await;

    let resp = s.pull_diagnostics("app.php").await;
    assert!(
        unguarded_messages(&resp).is_empty(),
        "Laravel's own default (no $fillable/$guarded declared) is fully guarded — must not be flagged"
    );
}

#[tokio::test]
async fn does_not_flag_force_fill_call() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "Models/Post.php",
        "<?php\nclass Post extends \\Illuminate\\Database\\Eloquent\\Model {\n    protected $guarded = [];\n}\n",
    )
    .await;
    s.open(
        "app.php",
        "<?php\nfunction handle(): void {\n    $p = new Post();\n    $p->forceFill(['title' => 'hi']);\n}\n",
    )
    .await;

    let resp = s.pull_diagnostics("app.php").await;
    assert!(
        unguarded_messages(&resp).is_empty(),
        "forceFill() deliberately bypasses guarding — must not be flagged"
    );
}

#[tokio::test]
async fn flags_create_call_through_custom_abstract_base_model() {
    let tmp = tempfile::tempdir().unwrap();
    write_minimal_laravel_project(tmp.path());
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "Models/BaseModel.php",
        "<?php\nabstract class BaseModel extends \\Illuminate\\Database\\Eloquent\\Model {\n    protected $guarded = [];\n}\n",
    )
    .await;
    s.open(
        "Models/Post.php",
        "<?php\nclass Post extends BaseModel {\n}\n",
    )
    .await;
    s.open(
        "app.php",
        "<?php\nfunction handle(): void {\n    Post::create(['title' => 'hi']);\n}\n",
    )
    .await;

    let resp = s.pull_diagnostics("app.php").await;
    let messages = unguarded_messages(&resp);
    assert_eq!(
        messages.len(),
        1,
        "guard declared on a custom abstract base class must still be walked transitively, got: {messages:?}"
    );
}

#[tokio::test]
async fn non_laravel_workspace_never_flags() {
    let tmp = tempfile::tempdir().unwrap();
    // Deliberately no `artisan` — not a Laravel workspace.
    let mut s = TestServer::with_root(tmp.path()).await;
    s.wait_for_index_ready().await;

    s.open(
        "Models/Post.php",
        "<?php\nclass Post extends \\Illuminate\\Database\\Eloquent\\Model {\n    protected $guarded = [];\n}\n",
    )
    .await;
    s.open(
        "app.php",
        "<?php\nfunction handle(): void {\n    Post::create(['title' => 'hi']);\n}\n",
    )
    .await;

    let resp = s.pull_diagnostics("app.php").await;
    assert!(
        unguarded_messages(&resp).is_empty(),
        "non-Laravel workspaces must never run this check"
    );
}
