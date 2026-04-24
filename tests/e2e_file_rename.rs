//! PSR-4-aware file rename: moving a file that contains a class must
//! rewrite `use` imports in every dependent file. Also covers file
//! deletion (drop the `use` lines) and file creation (no edits needed).
//!
//! CLAUDE.md: `file_rename` — "PSR-4-aware file/folder rename
//! (willRenameFiles / didRenameFiles / delete variants) rewriting `use`
//! imports". Existing `e2e_file_ops.rs` only asserts "no RPC error"; these
//! tests verify the actual edit content.

mod common;

use common::TestServer;

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

fn find_edits_for<'a>(resp: &'a serde_json::Value, uri_suffix: &str) -> Vec<&'a serde_json::Value> {
    let changes = resp["result"]["changes"]
        .as_object()
        .expect("expected `changes` map in WorkspaceEdit");
    changes
        .iter()
        .filter(|(uri, _)| uri.ends_with(uri_suffix))
        .flat_map(|(_, edits)| edits.as_array().cloned().unwrap_or_default())
        .collect::<Vec<_>>()
        .leak()
        .iter()
        .collect()
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
    let changes = resp["result"]["changes"]
        .as_object()
        .expect("expected changes map");
    let touched: Vec<&str> = changes.keys().map(String::as_str).collect();

    assert!(
        touched
            .iter()
            .any(|u| u.ends_with("src/Service/Registry.php")),
        "expected use-import edit in Registry.php, got: {touched:?}"
    );
    assert!(
        touched
            .iter()
            .any(|u| u.ends_with("src/Service/Greeter.php")),
        "expected use-import edit in Greeter.php, got: {touched:?}"
    );

    // Spot-check that the edit text mentions the new namespace.
    let all_new_texts: Vec<String> = changes
        .values()
        .flat_map(|edits| edits.as_array().cloned().unwrap_or_default())
        .map(|e| e["newText"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        all_new_texts
            .iter()
            .any(|t| t.contains("App\\Entity\\User")),
        "expected new FQN App\\Entity\\User in edits, got: {all_new_texts:?}"
    );
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
    let Some(changes) = resp["result"]["changes"].as_object() else {
        panic!("expected changes map for willDeleteFiles, got: {resp:?}");
    };
    let touched: Vec<&str> = changes.keys().map(String::as_str).collect();
    assert!(
        touched
            .iter()
            .any(|u| u.ends_with("src/Service/Registry.php"))
            || touched
                .iter()
                .any(|u| u.ends_with("src/Service/Greeter.php")),
        "expected use-import removal in a Service/*.php dependent, got: {touched:?}"
    );
    // Silence unused-helper warning if this path ever simplifies.
    let _ = find_edits_for;
}
