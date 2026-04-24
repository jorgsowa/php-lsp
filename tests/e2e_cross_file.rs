//! Cross-file navigation tests over a minimal PSR-4 workspace.
//!
//! The fixture `psr4-mini` defines `App\Model\User`, `App\Service\Registry`,
//! and `App\Service\Greeter` with `use` imports linking them together. These
//! tests exercise the workspace index + PSR-4 autoload resolution end-to-end.

mod common;

use common::TestServer;

async fn bring_up() -> TestServer {
    let mut server = TestServer::with_fixture("psr4-mini").await;
    server.wait_for_index_ready().await;
    server
}

/// Open a fixture file via the LSP wire protocol. Many handlers (goto-def,
/// rename) require the file to be in the document store, not merely in the
/// workspace index — this loads the on-disk content and sends `didOpen`.
async fn open_fixture(server: &mut TestServer, path: &str) {
    let (text, _, _) = server.locate(path, "<?php", 0);
    server.open(path, &text).await;
}

/// Goto-definition on `User` inside `Greeter::greet` parameter must jump
/// across files to `src/Model/User.php` via the `use App\Model\User` import.
#[tokio::test]
async fn goto_definition_resolves_use_import_across_files() {
    let mut server = bring_up().await;
    open_fixture(&mut server, "src/Service/Greeter.php").await;
    let (_, line, ch) = server.locate("src/Service/Greeter.php", "User $user", 0);

    let resp = server.definition("src/Service/Greeter.php", line, ch).await;
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected cross-file definition: {resp:?}"
    );
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    let uri = loc["uri"].as_str().unwrap();
    assert!(
        uri.ends_with("src/Model/User.php"),
        "definition must resolve to User.php, got: {uri}"
    );
}

/// Goto-definition on a method call across files: `$user->greeting()` in
/// Greeter must jump to `User::greeting` in Model/User.php.
#[tokio::test]
async fn goto_definition_method_call_across_files() {
    let mut server = bring_up().await;
    open_fixture(&mut server, "src/Service/Greeter.php").await;
    let (_, line, ch) = server.locate("src/Service/Greeter.php", "greeting()", 0);

    let resp = server.definition("src/Service/Greeter.php", line, ch).await;
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "expected cross-file method definition: {resp:?}"
    );
    let loc = if result.is_array() {
        &result[0]
    } else {
        result
    };
    assert!(
        loc["uri"].as_str().unwrap().ends_with("src/Model/User.php"),
        "method definition must land in User.php, got: {loc:?}"
    );
}

/// Find-references on `User` at its declaration site must surface both
/// `use App\Model\User` imports (in Registry and Greeter) plus the param
/// types. This is the safety-critical path rename depends on.
#[tokio::test]
async fn references_include_use_imports_across_files() {
    let mut server = bring_up().await;
    // Open files so they're in the live index (belt-and-suspenders — the
    // workspace scan should already have indexed them).
    server
        .open(
            "src/Model/User.php",
            &std::fs::read_to_string(std::path::Path::new(
                &server.uri("src/Model/User.php").replace("file://", ""),
            ))
            .unwrap_or_default(),
        )
        .await;

    let (_, line, ch) = server.locate("src/Model/User.php", "class User", 0);
    // `class User` — cursor on the `U` of User (after "class ")
    let resp = server
        .references("src/Model/User.php", line, ch + 6, false)
        .await;

    let refs = resp["result"].as_array().expect("references array");
    let ref_uris: Vec<&str> = refs.iter().filter_map(|r| r["uri"].as_str()).collect();
    assert!(
        ref_uris
            .iter()
            .any(|u| u.ends_with("src/Service/Registry.php")),
        "expected a reference in Registry.php, got: {ref_uris:?}"
    );
    assert!(
        ref_uris
            .iter()
            .any(|u| u.ends_with("src/Service/Greeter.php")),
        "expected a reference in Greeter.php, got: {ref_uris:?}"
    );
}

/// Rename across files must produce edits in every file that uses the
/// renamed symbol (declaration + `use` imports + type hints).
#[tokio::test]
async fn rename_class_edits_all_dependents() {
    let mut server = bring_up().await;
    open_fixture(&mut server, "src/Model/User.php").await;
    open_fixture(&mut server, "src/Service/Registry.php").await;
    open_fixture(&mut server, "src/Service/Greeter.php").await;
    let (_, line, ch) = server.locate("src/Model/User.php", "class User", 0);

    let resp = server
        .rename("src/Model/User.php", line, ch + 6, "Account")
        .await;

    assert!(resp["error"].is_null(), "rename error: {resp:?}");
    let changes = resp["result"]["changes"]
        .as_object()
        .expect("rename must return `changes` map");

    // Must touch at least User.php, Registry.php, Greeter.php.
    let touched: Vec<&String> = changes.keys().collect();
    let ends_with = |suffix: &str| touched.iter().any(|u| u.ends_with(suffix));
    assert!(
        ends_with("src/Model/User.php"),
        "rename must edit the declaration file, got: {touched:?}"
    );
    assert!(
        ends_with("src/Service/Registry.php"),
        "rename must edit Registry.php (use + @var + param), got: {touched:?}"
    );
    assert!(
        ends_with("src/Service/Greeter.php"),
        "rename must edit Greeter.php (use + param), got: {touched:?}"
    );
}

/// Workspace symbol search must find `User` by short name even though the
/// FQN is `App\Model\User`.
#[tokio::test]
async fn workspace_symbol_finds_class_by_short_name() {
    let mut server = bring_up().await;
    let resp = server.workspace_symbols("User").await;
    let symbols = resp["result"].as_array().expect("symbols array");
    assert!(
        symbols.iter().any(|s| s["name"].as_str() == Some("User")
            || s["name"]
                .as_str()
                .map(|n| n.contains("User"))
                .unwrap_or(false)),
        "workspace symbol `User` must appear, got: {symbols:?}"
    );
}
