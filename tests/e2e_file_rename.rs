//! PSR-4-aware file rename: moving a file that contains a class must
//! rewrite `use` imports in every dependent file. Also covers file
//! deletion (drop the `use` lines) and file creation (no edits needed).
//!
//! CLAUDE.md: `file_rename` — "PSR-4-aware file/folder rename
//! (willRenameFiles / didRenameFiles / delete variants) rewriting `use`
//! imports". Existing `e2e_file_ops.rs` only asserts "no RPC error"; these
//! tests verify the actual edit content.

mod common;

use common::{TestServer, canonicalize_workspace_edit};
use expect_test::expect;

async fn bring_up() -> TestServer {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;
    // Open the files whose imports should be rewritten so they're in the
    // live document store (matching the pattern in e2e_cross_file.rs).
    let (user, _, _) = server.locate("src/Model/User.php", "<?php", 0);
    server.open("src/Model/User.php", &user).await;
    let (reg, _, _) = server.locate("src/Service/Registry.php", "<?php", 0);
    server.open("src/Service/Registry.php", &reg).await;
    let (greet, _, _) = server.locate("src/Service/Greeter.php", "<?php", 0);
    server.open("src/Service/Greeter.php", &greet).await;
    server
}

/// Moving `src/Model/User.php` to `src/Entity/User.php` changes the class's
/// FQN from `App\Model\User` to `App\Entity\User`. Every `use App\Model\User`
/// in dependent files must be rewritten.
#[tokio::test]
async fn will_rename_file_rewrites_use_imports_in_dependents() {
    let mut server = bring_up().await;

    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Entity/User.php");

    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;

    assert!(resp["error"].is_null(), "willRenameFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    // Snapshot the full edit so byte-offset regressions in the `use`-import
    // rewriter are caught immediately. Run `UPDATE_EXPECT=1 cargo test` if
    // the rewriter output changes intentionally.
    expect![[r#"
        // src/Service/Greeter.php
        4:4-4:18 → "App\\Entity\\User"

        // src/Service/Registry.php
        4:4-4:18 → "App\\Entity\\User""#]]
    .assert_eq(&snap);
}

/// Renaming a file to a path with the same PSR-4-derived FQN (same class
/// name, same namespace) must be a no-op — no edits produced.
#[tokio::test]
async fn will_rename_file_same_psr4_fqn_produces_no_edits() {
    let mut server = bring_up().await;

    let old_uri = server.uri("src/Model/User.php");
    // Same directory, different name would change the FQN — instead rename
    // to itself (noop) which is the degenerate case.
    let new_uri = old_uri.clone();

    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;
    assert!(resp["error"].is_null(), "willRenameFiles error: {resp:?}");
    let changes = resp["result"]["changes"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    assert!(
        changes.is_empty(),
        "rename-to-self must not produce edits, got: {changes:?}"
    );
}

/// Deleting a file that defines a class must produce edits stripping the
/// `use App\Model\User;` lines from every dependent.
#[tokio::test]
async fn will_delete_file_strips_use_imports_from_dependents() {
    let mut server = bring_up().await;

    let uri = server.uri("src/Model/User.php");
    let resp = server.will_delete_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willDeleteFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Service/Greeter.php
        4:0-5:0 → ""

        // src/Service/Registry.php
        4:0-5:0 → """#]]
    .assert_eq(&snap);
}
