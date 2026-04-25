//! Tests for `workspace/willCreateFiles` stub generation.
//!
//! The handler generates PSR-4-aware PHP class stubs when creating new files:
//! - PSR-4-mapped path → full `namespace` + `class` stub
//! - Root-namespace PSR-4 mapping (empty prefix) → `class` stub without `namespace`
//! - Path outside PSR-4 roots → minimal `<?php\n\n` stub
//! - Multiple files in one request → a stub edit for each file

mod common;

use common::{TestServer, canonicalize_workspace_edit};
use expect_test::expect;

// ── Test 1: PSR-4-mapped file gets namespace + class stub ────────────────────

/// A file created under `src/Model/` maps to `App\Model\Product`.
/// The server should return a workspace edit inserting a full stub with
/// `declare(strict_types=1)`, `namespace App\Model;`, and `class Product`.
#[tokio::test]
async fn will_create_files_psr4_mapped_generates_namespace_stub() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let uri = server.uri("src/Model/Product.php");
    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Model/Product.php
        0:0-0:0 → "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Model;\n\nclass Product\n{\n}\n""#]]
    .assert_eq(&snap);
}

// ── Test 2: File outside PSR-4 root gets minimal stub ───────────────────────

/// A file at `scripts/bootstrap.php` is not under `src/` so it has no
/// PSR-4 FQN. The server should return the fallback `<?php\n\n` stub.
#[tokio::test]
async fn will_create_files_outside_psr4_root_generates_minimal_stub() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let uri = server.uri("scripts/bootstrap.php");
    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    // The result must be a workspace edit with exactly one change containing
    // the minimal stub text.
    let changes = resp["result"]["changes"]
        .as_object()
        .expect("expected a changes map");
    assert_eq!(changes.len(), 1, "expected exactly one file in changes");

    let edits = changes.values().next().unwrap().as_array().unwrap();
    assert_eq!(edits.len(), 1);
    let new_text = edits[0]["newText"].as_str().unwrap();
    assert_eq!(
        new_text, "<?php\n\n",
        "expected minimal stub for non-PSR-4 path"
    );
}

// ── Test 3: Root-namespace PSR-4 mapping produces stub without namespace ─────

/// A fixture with `"": "src/"` maps every file under `src/` to a root-namespace
/// class (no namespace prefix). The stub must include `declare(strict_types=1)`
/// and `class Bootstrap` but no `namespace` line.
#[tokio::test]
async fn will_create_files_root_namespace_generates_stub_without_namespace() {
    let mut server = TestServer::with_fixture("psr4-root").await;
    server.wait_for_index_ready().await;

    let uri = server.uri("src/Bootstrap.php");
    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Bootstrap.php
        0:0-0:0 → "<?php\n\ndeclare(strict_types=1);\n\nclass Bootstrap\n{\n}\n""#]]
    .assert_eq(&snap);
}

// ── Test 4: Multiple files in one request get independent stubs ──────────────

/// Sending two URIs in one `willCreateFiles` request must produce two
/// independent workspace-edit entries — one stub per file.
#[tokio::test]
async fn will_create_files_multiple_files_get_independent_stubs() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let uri_a = server.uri("src/Alpha.php");
    let uri_b = server.uri("src/Beta.php");
    let resp = server.will_create_files(vec![uri_a, uri_b]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Alpha.php
        0:0-0:0 → "<?php\n\ndeclare(strict_types=1);\n\nnamespace App;\n\nclass Alpha\n{\n}\n"

        // src/Beta.php
        0:0-0:0 → "<?php\n\ndeclare(strict_types=1);\n\nnamespace App;\n\nclass Beta\n{\n}\n""#]]
    .assert_eq(&snap);
}
