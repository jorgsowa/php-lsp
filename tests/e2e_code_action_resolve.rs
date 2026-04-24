//! Code actions: deferred resolve must materialize the actual edit.
//!
//! CLAUDE.md flags that PHPDoc / Implement / Constructor / Getters / Setters
//! are tagged with `php_lsp_resolve` so the menu renders instantly, then the
//! client calls `codeAction/resolve` to fetch the edit. Existing
//! `e2e_code_actions.rs` only verifies the action is *offered* — these tests
//! drive the second half of the round-trip and check the resolved edit is
//! non-empty and lands in the right file.

mod common;

use common::TestServer;
use serde_json::Value;

fn find_action_by<'a>(resp: &'a Value, pred: impl Fn(&str) -> bool) -> Option<&'a Value> {
    resp["result"]
        .as_array()?
        .iter()
        .find(|a| a["title"].as_str().map(&pred).unwrap_or(false))
}

async fn resolve(server: &mut TestServer, action: &Value) -> Value {
    server
        .client()
        .request("codeAction/resolve", action.clone())
        .await
}

fn edits_for_uri<'a>(workspace_edit: &'a Value, uri: &str) -> Vec<&'a Value> {
    workspace_edit["changes"][uri]
        .as_array()
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

/// Resolving the "Add PHPDoc" action must return a WorkspaceEdit that
/// inserts a `/**` comment block above the function.
#[tokio::test]
async fn resolve_phpdoc_action_inserts_docblock() {
    let mut server = TestServer::new().await;
    let path = "rp_phpdoc.php";
    server
        .open(path, "<?php\nfunction noDoc(int $x): int { return $x; }\n")
        .await;
    let uri = server.uri(path);

    let resp = server.code_action(path, 1, 9, 1, 14).await;
    let action = find_action_by(&resp, |t| t.to_lowercase().contains("phpdoc"))
        .expect("PHPDoc action not offered")
        .clone();

    let resolved = resolve(&mut server, &action).await;
    assert!(resolved["error"].is_null(), "resolve errored: {resolved:?}");

    let edit = &resolved["result"]["edit"];
    assert!(
        !edit.is_null(),
        "resolved action must have `edit`: {resolved:?}"
    );
    let edits = edits_for_uri(edit, &uri);
    assert!(!edits.is_empty(), "resolved edits empty: {edit:?}");
    let any_docblock = edits
        .iter()
        .any(|e| e["newText"].as_str().unwrap_or_default().contains("/**"));
    assert!(
        any_docblock,
        "PHPDoc resolve must insert a `/**` block, got: {edits:?}"
    );
}

/// Resolving "Generate constructor" must insert a `__construct` method
/// referencing the class properties.
#[tokio::test]
async fn resolve_generate_constructor_inserts_constructor() {
    let mut server = TestServer::new().await;
    let path = "rp_ctor.php";
    server
        .open(
            path,
            "<?php\nclass Point {\n    public int $x;\n    public int $y;\n}\n",
        )
        .await;
    let uri = server.uri(path);

    let resp = server.code_action(path, 1, 6, 1, 11).await;
    let action = find_action_by(&resp, |t| t.to_lowercase().contains("constructor"))
        .expect("Generate constructor action not offered")
        .clone();

    let resolved = resolve(&mut server, &action).await;
    assert!(resolved["error"].is_null(), "resolve errored: {resolved:?}");

    let edit = &resolved["result"]["edit"];
    assert!(!edit.is_null(), "resolved action must have `edit`");
    let edits = edits_for_uri(edit, &uri);
    let joined: String = edits
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("__construct"),
        "resolved edit must contain `__construct`, got: {joined}"
    );
    assert!(
        joined.contains("$x") && joined.contains("$y"),
        "constructor must reference both properties, got: {joined}"
    );
}

/// Resolving "Implement missing methods" on a class that implements an
/// interface must insert a stub for every unimplemented method.
#[tokio::test]
async fn resolve_implement_missing_inserts_method_stubs() {
    let mut server = TestServer::new().await;
    let path = "rp_impl.php";
    server
        .open(
            path,
            "<?php\ninterface Greetable {\n    public function greet(): string;\n    public function farewell(): string;\n}\nclass Hello implements Greetable {\n}\n",
        )
        .await;
    let uri = server.uri(path);

    let resp = server.code_action(path, 5, 0, 5, 0).await;
    let action = find_action_by(&resp, |t| t.to_lowercase().contains("implement"))
        .expect("Implement action not offered")
        .clone();

    let resolved = resolve(&mut server, &action).await;
    assert!(resolved["error"].is_null(), "resolve errored: {resolved:?}");

    let edit = &resolved["result"]["edit"];
    assert!(!edit.is_null(), "resolved action must have `edit`");
    let edits = edits_for_uri(edit, &uri);
    let joined: String = edits
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("greet") && joined.contains("farewell"),
        "both interface methods must be stubbed, got: {joined}"
    );
}

/// Eager actions (Extract variable) return edits inline, no resolve needed.
/// Verify the returned CodeAction already carries an `edit`.
#[tokio::test]
async fn eager_extract_variable_edit_is_inline() {
    let mut server = TestServer::new().await;
    let path = "rp_extract.php";
    server.open(path, "<?php\n$result = 1 + 2;\n").await;

    let resp = server.code_action(path, 1, 10, 1, 15).await;
    let action = find_action_by(&resp, |t| t.to_lowercase().contains("extract"))
        .expect("Extract action not offered");

    assert!(
        !action["edit"].is_null(),
        "eager Extract action must carry edit inline, got: {action:?}"
    );
}
