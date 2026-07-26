//! File operation stubs: willRenameFiles, willCreateFiles, willDeleteFiles.
//! Covers both rootless servers (no PSR-4 map) and PSR-4-aware fixture workspaces.

use super::*;

use expect_test::expect;

// ── rootless server (no PSR-4 map) ──────────────────────────────────────────

#[tokio::test]
async fn will_rename_files_outside_psr4_returns_null() {
    let mut server = TestServer::new().await;
    server
        .open("rename_old.php", "<?php\nclass OldClass {}\n")
        .await;

    let old_uri = server.uri("rename_old.php");
    let new_uri = server.uri("rename_new.php");

    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;

    assert!(resp["error"].is_null(), "willRenameFiles error: {:?}", resp);
    assert!(
        resp["result"].is_null(),
        "expected null (no PSR-4 map → no edits), got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn will_create_files_returns_workspace_edit_with_stub() {
    let mut server = TestServer::new().await;
    let uri = server.uri("new_created.php");

    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {:?}", resp);
    let snap = canonicalize_workspace_edit(&resp["result"], &server.uri(""));
    expect![[r#"
        // new_created.php
        0:0-0:0 → "<?php\n\n""#]]
    .assert_eq(&snap);
}

#[tokio::test]
async fn will_delete_files_outside_psr4_returns_null() {
    let mut server = TestServer::new().await;
    server
        .open("to_delete.php", "<?php\nclass ToDelete {}\n")
        .await;

    let uri = server.uri("to_delete.php");

    let resp = server.will_delete_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willDeleteFiles error: {:?}", resp);
    assert!(
        resp["result"].is_null(),
        "expected null (no PSR-4 map → no use-sites to remove), got: {:?}",
        resp["result"]
    );
}

#[tokio::test]
async fn will_rename_files_rewrites_use_statements_in_dependents() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Model/Account.php");
    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;

    assert!(resp["error"].is_null(), "willRenameFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // src/Model/User.php
        4:6-4:10 → "Account"

        // src/Service/Greeter.php
        4:4-4:18 → "App\\Model\\Account"
        8:26-8:30 → "Account"

        // src/Service/Registry.php
        4:4-4:18 → "App\\Model\\Account"
        11:29-11:33 → "Account""#]]
    .assert_eq(&snap);
}

#[tokio::test]
async fn will_delete_files_removes_use_statements_in_dependents() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

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

// ── PSR-4-aware stub generation (psr4-mini fixture) ─────────────────────────

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

#[tokio::test]
async fn will_create_files_outside_psr4_root_generates_minimal_stub() {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;

    let uri = server.uri("scripts/bootstrap.php");
    let resp = server.will_create_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willCreateFiles error: {resp:?}");
    let root = server.uri("");
    let snap = canonicalize_workspace_edit(&resp["result"], &root);
    expect![[r#"
        // scripts/bootstrap.php
        0:0-0:0 → "<?php\n\n""#]]
    .assert_eq(&snap);
}

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

/// The willCreateFiles handler is only reachable when the capability is
/// advertised — spec-compliant clients gate on it. Regression pin for the
/// capability registration (the handler itself is tested above).
#[tokio::test]
async fn file_operation_capabilities_include_will_create() {
    let (_server, resp) = TestServer::new_with_options(serde_json::json!({})).await;
    let ops = &resp["result"]["capabilities"]["workspace"]["fileOperations"];
    for cap in [
        "willCreate",
        "didCreate",
        "willRename",
        "didRename",
        "willDelete",
        "didDelete",
    ] {
        assert!(
            ops[cap].is_object(),
            "missing workspace.fileOperations.{cap} capability, got: {ops}"
        );
    }
}

// ── importer-lookup regression pins ──────────────────────────────────────────
//
// willRename/willDelete `use`-line rewrites resolve their candidate files from
// the workspace index's recorded imports (`files_importing`), not a workspace
// text scan + parse. These pin the behaviors that lookup must preserve.

/// Build a tempdir PSR-4 workspace (`App\` → `src/`) from `(path, text)` pairs.
fn psr4_workspace(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("composer.json"),
        r#"{ "autoload": { "psr-4": { "App\\": "src/" } } }"#,
    )
    .unwrap();
    for (path, text) in files {
        let full = dir.path().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }
    dir
}

/// An aliased import (`use App\Model\User as Person;`) must still be found by
/// the importer lookup — the index records the FQN, not the alias — and its
/// `use` line rewritten on rename. The decoy file mentions `User` only in a
/// comment and a same-short-name string: under the old text scan it was
/// parsed as a candidate; now it must simply produce no edits.
#[tokio::test]
async fn will_rename_files_rewrites_aliased_import() {
    let dir = psr4_workspace(&[
        (
            "src/Model/User.php",
            "<?php\n\nnamespace App\\Model;\n\nclass User\n{\n}\n",
        ),
        (
            "src/Consumer.php",
            "<?php\n\nnamespace App;\n\nuse App\\Model\\User as Person;\n\nclass Consumer\n{\n    public function make(): Person\n    {\n        return new Person();\n    }\n}\n",
        ),
        (
            "src/Decoy.php",
            "<?php\n\nnamespace App;\n\n// User is renamed elsewhere; $user strings only\nclass Decoy\n{\n    public string $note = 'User';\n}\n",
        ),
    ]);
    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Model/Account.php");
    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;

    assert!(resp["error"].is_null(), "willRenameFiles error: {resp:?}");
    let snap = canonicalize_workspace_edit(&resp["result"], &server.uri(""));
    expect![[r#"
        // src/Consumer.php
        4:4-4:18 → "App\\Model\\Account"

        // src/Model/User.php
        4:6-4:10 → "Account""#]]
    .assert_eq(&snap);
}

/// Deleting an aliased-import target must remove the whole `use ... as ...;`
/// line, found via the importer lookup.
#[tokio::test]
async fn will_delete_files_removes_aliased_import_line() {
    let dir = psr4_workspace(&[
        (
            "src/Model/User.php",
            "<?php\n\nnamespace App\\Model;\n\nclass User\n{\n}\n",
        ),
        (
            "src/Consumer.php",
            "<?php\n\nnamespace App;\n\nuse App\\Model\\User as Person;\n\nclass Consumer\n{\n}\n",
        ),
    ]);
    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    let uri = server.uri("src/Model/User.php");
    let resp = server.will_delete_files(vec![uri]).await;

    assert!(resp["error"].is_null(), "willDeleteFiles error: {resp:?}");
    let snap = canonicalize_workspace_edit(&resp["result"], &server.uri(""));
    expect![[r#"
        // src/Consumer.php
        4:0-5:0 → """#]]
    .assert_eq(&snap);
}

/// Known-gap pin: group `use App\Model\{User};` lines are not rewritten by
/// the text-level `use` editor (before or after the importer-lookup change) —
/// note the snapshot has no line-4 edit for Grouped.php. Reference sites and
/// the declaration are still renamed via mir's postings, so the import line
/// is the one stale remnant. If a `4:…` edit ever appears here, the gap was
/// closed — update the docs alongside it.
#[tokio::test]
async fn will_rename_files_group_use_import_pins_known_gap() {
    let dir = psr4_workspace(&[
        (
            "src/Model/User.php",
            "<?php\n\nnamespace App\\Model;\n\nclass User\n{\n}\n",
        ),
        (
            "src/Grouped.php",
            "<?php\n\nnamespace App;\n\nuse App\\Model\\{User};\n\nclass Grouped\n{\n    public function make(): User\n    {\n        return new User();\n    }\n}\n",
        ),
    ]);
    let mut server = TestServer::with_root(dir.path()).await;
    server.wait_for_index_ready().await;

    let old_uri = server.uri("src/Model/User.php");
    let new_uri = server.uri("src/Model/Account.php");
    let resp = server.will_rename_files(vec![(old_uri, new_uri)]).await;

    assert!(resp["error"].is_null(), "willRenameFiles error: {resp:?}");
    let snap = canonicalize_workspace_edit(&resp["result"], &server.uri(""));
    expect![[r#"
        // src/Grouped.php
        8:28-8:32 → "Account"
        10:19-10:23 → "Account"

        // src/Model/User.php
        4:6-4:10 → "Account""#]]
    .assert_eq(&snap);
}
