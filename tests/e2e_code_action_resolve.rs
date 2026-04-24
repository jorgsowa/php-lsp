//! Code actions: deferred resolve must materialize the actual edit.
//!
//! CLAUDE.md flags that PHPDoc / Implement / Constructor / Getters / Setters
//! are tagged with `php_lsp_resolve` so the menu renders instantly, then the
//! client calls `codeAction/resolve` to fetch the edit. Existing
//! `e2e_code_actions.rs` only verifies the action is *offered* — these tests
//! drive the second half of the round-trip and check the resolved edit lands
//! in the right file with correct content.

mod common;

use common::TestServer;
use expect_test::expect;
use serde_json::Value;

/// Find the first action whose title starts with `prefix` (case-insensitive).
/// Prefix matching is tighter than `contains` — "Extract variable" won't
/// match against "Extract method" or "Extract constant".
fn find_action_starting_with<'a>(resp: &'a Value, prefix: &str) -> Option<&'a Value> {
    let prefix_lower = prefix.to_lowercase();
    resp["result"].as_array()?.iter().find(|a| {
        a["title"]
            .as_str()
            .map(|t| t.to_lowercase().starts_with(&prefix_lower))
            .unwrap_or(false)
    })
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
    let action = find_action_starting_with(&resp, "add phpdoc")
        .or_else(|| find_action_starting_with(&resp, "generate phpdoc"))
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
    let action = find_action_starting_with(&resp, "generate constructor")
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
    let action = find_action_starting_with(&resp, "implement")
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
/// Verify the edit is present, lands in the right file, and produces two
/// sub-edits: one inserting `$name = 1 + 2;` above the original line, and
/// one replacing the selection with the new variable reference.
#[tokio::test]
async fn eager_extract_variable_produces_correct_edits() {
    let mut server = TestServer::new().await;
    let path = "rp_extract.php";
    server.open(path, "<?php\n$result = 1 + 2;\n").await;
    let uri = server.uri(path);

    // Selection is "1 + 2" (cols 10..15 on line 1).
    let resp = server.code_action(path, 1, 10, 1, 15).await;
    let action = find_action_starting_with(&resp, "extract variable")
        .expect("Extract variable action not offered");

    assert!(
        !action["edit"].is_null(),
        "eager Extract action must carry edit inline, got: {action:?}"
    );

    let edits = edits_for_uri(&action["edit"], &uri);
    assert_eq!(
        edits.len(),
        2,
        "extract variable must produce 2 edits (insert + replace), got: {edits:?}"
    );

    let replacement = edits
        .iter()
        .find(|e| {
            let s = e["range"]["start"].clone();
            let en = e["range"]["end"].clone();
            s["line"].as_u64() == Some(1)
                && s["character"].as_u64() == Some(10)
                && en["line"].as_u64() == Some(1)
                && en["character"].as_u64() == Some(15)
        })
        .expect("expected a replace edit covering cols 10..15 on line 1");
    let replacement_text = replacement["newText"].as_str().unwrap_or_default();
    assert!(
        replacement_text.starts_with('$'),
        "replacement must substitute a variable reference, got: {replacement_text:?}"
    );

    let insertion = edits
        .iter()
        .find(|e| *e != replacement)
        .expect("expected a second (insertion) edit");
    let insert_text = insertion["newText"].as_str().unwrap_or_default();
    assert!(
        insert_text.contains("1 + 2"),
        "insertion must carry the extracted expression, got: {insert_text:?}"
    );
    assert!(
        insert_text.contains(replacement_text),
        "the inserted `$var = ...;` and the replacement `$var` must share the variable name"
    );
}

/// Snapshot-style check on the Generate constructor output shape. Pins the
/// *structure* of the generated method (visibility, param list shape, body)
/// rather than brittle whitespace. rust-analyzer uses expect_test for this.
#[tokio::test]
async fn generate_constructor_matches_snapshot() {
    let mut server = TestServer::new().await;
    let path = "snap_ctor.php";
    server
        .open(
            path,
            "<?php\nclass Point {\n    public int $x;\n    public int $y;\n}\n",
        )
        .await;
    let uri = server.uri(path);

    let resp = server.code_action(path, 1, 6, 1, 11).await;
    let action = find_action_starting_with(&resp, "generate constructor")
        .expect("Generate constructor action not offered")
        .clone();
    let resolved = resolve(&mut server, &action).await;

    let edits = edits_for_uri(&resolved["result"]["edit"], &uri);
    let body: String = edits
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n---\n");

    // Skeleton check — constructor signature and assignments, regardless of
    // exact whitespace choices. If the generator shape changes, update here.
    let skeleton: String = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    expect![[r#"
        public function __construct(
        int $x,
        int $y,
        ) {
        $this->x = $x;
        $this->y = $y;
        }"#]]
    .assert_eq(&skeleton);
}
