//! Code actions: deferred resolve must materialize the actual edit.
//!
//! CLAUDE.md flags that PHPDoc / Implement / Constructor / Getters / Setters
//! are tagged with `php_lsp_resolve` so the menu renders instantly, then the
//! client calls `codeAction/resolve` to fetch the edit. Existing
//! `e2e_code_actions.rs` only verifies the action is *offered* — these tests
//! drive the second half of the round-trip and check the resolved edit lands
//! in the right file with correct content.
//!
//! The selection range is marked inline with two `$0` markers so it's visible
//! in the fixture source.

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

/// Open `fixture`, run codeAction over the `$0…$0` selection, and find the
/// first action whose title starts with `title_prefix`. Returns (uri, action).
async fn action_at_selection(
    server: &mut TestServer,
    fixture: &str,
    title_prefix: &str,
) -> (String, Value) {
    let opened = server.open_fixture(fixture).await;
    let r = opened.range().clone();
    let uri = server.uri(&r.path);
    let resp = server.code_action_at(&r).await;
    let action = find_action_starting_with(&resp, title_prefix)
        .unwrap_or_else(|| panic!("`{title_prefix}` action not offered: {resp:?}"))
        .clone();
    (uri, action)
}

/// Resolving "Add PHPDoc" must return a WorkspaceEdit that inserts a `/**`
/// comment block above the function.
#[tokio::test]
async fn resolve_phpdoc_action_inserts_docblock() {
    let mut server = TestServer::new().await;
    let (uri, action) = action_at_selection(
        &mut server,
        r#"<?php
function $0noDoc$0(int $x): int { return $x; }
"#,
        "generate phpdoc",
    )
    .await;

    let resolved = resolve(&mut server, &action).await;
    assert!(resolved["error"].is_null(), "resolve errored: {resolved:?}");

    let edit = &resolved["result"]["edit"];
    assert!(
        !edit.is_null(),
        "resolved action must have `edit`: {resolved:?}"
    );
    let edits = edits_for_uri(edit, &uri);
    assert!(!edits.is_empty(), "resolved edits empty: {edit:?}");
    assert!(
        edits
            .iter()
            .any(|e| e["newText"].as_str().unwrap_or_default().contains("/**")),
        "PHPDoc resolve must insert a `/**` block: {edits:?}"
    );
}

/// Resolving "Generate constructor" must insert a `__construct` method
/// referencing the class properties.
#[tokio::test]
async fn resolve_generate_constructor_inserts_constructor() {
    let mut server = TestServer::new().await;
    let (uri, action) = action_at_selection(
        &mut server,
        r#"<?php
class $0Point$0 {
    public int $x;
    public int $y;
}
"#,
        "generate constructor",
    )
    .await;

    let resolved = resolve(&mut server, &action).await;
    assert!(resolved["error"].is_null(), "resolve errored: {resolved:?}");

    let edit = &resolved["result"]["edit"];
    assert!(!edit.is_null(), "resolved action must have `edit`");
    let joined: String = edits_for_uri(edit, &uri)
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("__construct"),
        "resolved edit must contain `__construct`: {joined}"
    );
    assert!(
        joined.contains("$x") && joined.contains("$y"),
        "constructor must reference both properties: {joined}"
    );
}

/// Resolving "Implement missing methods" on a class that implements an
/// interface must insert a stub for every unimplemented method.
#[tokio::test]
async fn resolve_implement_missing_inserts_method_stubs() {
    let mut server = TestServer::new().await;
    // Empty selection inside the class body.
    let (uri, action) = action_at_selection(
        &mut server,
        r#"<?php
interface Greetable {
    public function greet(): string;
    public function farewell(): string;
}
class Hello implements Greetable {
$0$0}
"#,
        "implement",
    )
    .await;

    let resolved = resolve(&mut server, &action).await;
    assert!(resolved["error"].is_null(), "resolve errored: {resolved:?}");

    let edit = &resolved["result"]["edit"];
    assert!(!edit.is_null(), "resolved action must have `edit`");
    let joined: String = edits_for_uri(edit, &uri)
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        joined.contains("greet") && joined.contains("farewell"),
        "both interface methods must be stubbed: {joined}"
    );
}

/// Eager actions (Extract variable) return edits inline, no resolve needed.
#[tokio::test]
async fn eager_extract_variable_produces_correct_edits() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
$result = $01 + 2$0;
"#,
        )
        .await;
    let r = opened.range().clone();
    let uri = server.uri(&r.path);

    let resp = server.code_action_at(&r).await;
    let action = find_action_starting_with(&resp, "extract variable")
        .expect("Extract variable action not offered");

    assert!(
        !action["edit"].is_null(),
        "eager Extract action must carry edit inline: {action:?}"
    );

    let edits = edits_for_uri(&action["edit"], &uri);
    assert_eq!(
        edits.len(),
        2,
        "extract variable must produce 2 edits (insert + replace): {edits:?}"
    );

    // One edit replaces the selected expression with a `$var` reference;
    // the other inserts `$var = 1 + 2;` above the line.
    let replacement = edits
        .iter()
        .find(|e| {
            e["range"]["start"]["line"].as_u64() == Some(r.start_line as u64)
                && e["range"]["start"]["character"].as_u64() == Some(r.start_character as u64)
                && e["range"]["end"]["line"].as_u64() == Some(r.end_line as u64)
                && e["range"]["end"]["character"].as_u64() == Some(r.end_character as u64)
        })
        .expect("expected a replace edit covering the $0…$0 selection");
    let replacement_text = replacement["newText"].as_str().unwrap_or_default();
    assert!(
        replacement_text.starts_with('$'),
        "replacement must substitute a variable reference: {replacement_text:?}"
    );

    let insertion = edits
        .iter()
        .find(|e| *e != replacement)
        .expect("expected a second (insertion) edit");
    let insert_text = insertion["newText"].as_str().unwrap_or_default();
    assert!(
        insert_text.contains("1 + 2"),
        "insertion must carry the extracted expression: {insert_text:?}"
    );
    assert!(
        insert_text.contains(replacement_text),
        "inserted `$var = ...;` and the replacement `$var` must share the variable name"
    );
}

/// Snapshot-style check on the Generate constructor output shape. Pins the
/// *structure* of the generated method (visibility, param list shape, body)
/// rather than brittle whitespace.
#[tokio::test]
async fn generate_constructor_matches_snapshot() {
    let mut server = TestServer::new().await;
    let (uri, action) = action_at_selection(
        &mut server,
        r#"<?php
class $0Point$0 {
    public int $x;
    public int $y;
}
"#,
        "generate constructor",
    )
    .await;
    let resolved = resolve(&mut server, &action).await;

    let body: String = edits_for_uri(&resolved["result"]["edit"], &uri)
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n---\n");

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

/// Eager Extract method: selection spans two statements inside a class method.
/// Edits must replace the selection with a `$this->…()` call and append a new
/// method carrying the extracted body.
#[tokio::test]
async fn eager_extract_method_produces_call_and_new_method() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
class Runner {
    public function run(): void {
$0        echo 'hello';
        echo 'world';
$0    }
}
"#,
        )
        .await;
    let r = opened.range().clone();
    let uri = server.uri(&r.path);

    let resp = server.code_action_at(&r).await;
    let action = find_action_starting_with(&resp, "extract method")
        .expect("Extract method action not offered");
    assert!(
        !action["edit"].is_null(),
        "eager Extract method must carry edit inline: {action:?}"
    );

    let texts: Vec<String> = edits_for_uri(&action["edit"], &uri)
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert!(
        texts.iter().any(|t| t.contains("$this->")),
        "one edit must replace the selection with a `$this->…()` call: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("echo 'hello'") && t.contains("echo 'world'")),
        "the new method body must contain the extracted statements: {texts:?}"
    );
}

/// Eager Extract constant: selecting a string literal must insert a
/// `private const …` inside the class and replace the literal with `self::…`.
#[tokio::test]
async fn eager_extract_constant_produces_decl_and_reference() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
class Greeter {
    public function greet(): string { return $0'hello world'$0; }
}
"#,
        )
        .await;
    let r = opened.range().clone();
    let uri = server.uri(&r.path);

    let resp = server.code_action_at(&r).await;
    let action = find_action_starting_with(&resp, "extract constant")
        .expect("Extract constant action not offered");
    assert!(
        !action["edit"].is_null(),
        "eager Extract constant must carry edit inline: {action:?}"
    );

    let edits = edits_for_uri(&action["edit"], &uri);
    assert_eq!(
        edits.len(),
        2,
        "extract constant must produce 2 edits (decl + reference): {edits:?}"
    );
    let texts: Vec<&str> = edits
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default())
        .collect();
    assert!(
        texts
            .iter()
            .any(|t| t.contains("private const") && t.contains("'hello world'")),
        "must insert a `private const … = 'hello world';` declaration: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.starts_with("self::")),
        "must replace the literal with a `self::…` reference: {texts:?}"
    );
}

/// Eager Inline variable: cursor on `$tmp` with one visible assignment must
/// delete the assignment line and substitute the RHS at each usage.
#[tokio::test]
async fn eager_inline_variable_substitutes_rhs_and_deletes_assignment() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function f(): int {
    $tmp = 1 + 2;
    return $t$0mp;
}
"#,
        )
        .await;
    let c = opened.cursor().clone();
    let uri = server.uri(&c.path);

    // Inline uses a zero-width range at the cursor.
    let resp = server
        .code_action(&c.path, c.line, c.character, c.line, c.character)
        .await;
    let action = find_action_starting_with(&resp, "inline variable")
        .expect("Inline variable action not offered");
    assert!(
        !action["edit"].is_null(),
        "eager Inline variable must carry edit inline: {action:?}"
    );

    let edits = edits_for_uri(&action["edit"], &uri);
    // One substitution + one deletion of the assignment line.
    assert!(
        edits.len() >= 2,
        "inline variable must produce ≥2 edits (substitute + delete): {edits:?}"
    );
    assert!(
        edits.iter().any(|e| e["newText"].as_str() == Some("1 + 2")),
        "one edit must substitute the variable with its RHS `1 + 2`: {edits:?}"
    );
    // Deletion edit covers the whole assignment line (start char 0 → next line char 0, empty newText).
    assert!(
        edits.iter().any(|e| {
            e["newText"].as_str() == Some("")
                && e["range"]["start"]["character"].as_u64() == Some(0)
                && e["range"]["end"]["line"].as_u64()
                    == Some(e["range"]["start"]["line"].as_u64().unwrap() + 1)
        }),
        "one edit must delete the entire `$tmp = …;` line: {edits:?}"
    );
}

/// Inline variable must refuse when the variable is assigned more than once —
/// the refactor is ambiguous and silently picking one RHS would drop a write.
#[tokio::test]
async fn inline_variable_refuses_on_multiple_assignments() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
function f(): int {
    $tmp = 1;
    $tmp = 2;
    return $t$0mp;
}
"#,
        )
        .await;
    let c = opened.cursor().clone();

    let resp = server
        .code_action(&c.path, c.line, c.character, c.line, c.character)
        .await;
    assert!(
        find_action_starting_with(&resp, "inline variable").is_none(),
        "Inline variable must NOT be offered with multiple assignments: {resp:?}"
    );
}

/// Eager Organize imports: must sort the `use` block and drop an unused import.
#[tokio::test]
async fn eager_organize_imports_sorts_and_drops_unused() {
    let mut server = TestServer::new().await;
    let opened = server
        .open_fixture(
            r#"<?php
use App\Zebra;
use App\Alpha;
use App\Unused;

new Alpha();
new Zebra();
"#,
        )
        .await;
    let path = opened.fixture.files[0].path.clone();
    let uri = server.uri(&path);

    // Organize imports is independent of the selection — any range works.
    let resp = server.code_action(&path, 0, 0, 0, 0).await;
    let action = find_action_starting_with(&resp, "organize imports")
        .expect("Organize imports action not offered");
    assert!(
        !action["edit"].is_null(),
        "Organize imports must carry edit inline: {action:?}"
    );

    let edits = edits_for_uri(&action["edit"], &uri);
    let new_text: String = edits
        .iter()
        .map(|e| e["newText"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("");
    let alpha = new_text.find("Alpha").expect("Alpha must be kept");
    let zebra = new_text.find("Zebra").expect("Zebra must be kept");
    assert!(alpha < zebra, "Alpha must sort before Zebra: {new_text:?}");
    assert!(
        !new_text.contains("Unused"),
        "unused import must be dropped: {new_text:?}"
    );
}

/// Deferred Promote constructor parameter: resolving must remove the
/// redundant property declaration and the `$this->x = $x` assignment, and
/// prefix the constructor param with a visibility modifier.
#[tokio::test]
async fn resolve_promote_constructor_param_produces_visibility_and_drops_decl() {
    let mut server = TestServer::new().await;
    let (uri, action) = action_at_selection(
        &mut server,
        r#"<?php
class $0Box$0 {
    private int $x;
    public function __construct(int $x) {
        $this->x = $x;
    }
}
"#,
        "promote",
    )
    .await;

    let resolved = resolve(&mut server, &action).await;
    assert!(resolved["error"].is_null(), "resolve errored: {resolved:?}");

    let edit = &resolved["result"]["edit"];
    assert!(!edit.is_null(), "resolved promote action must have `edit`");
    let edits = edits_for_uri(edit, &uri);
    let has_visibility = edits.iter().any(|e| {
        e["newText"]
            .as_str()
            .map(|t| t.contains("private"))
            .unwrap_or(false)
    });
    assert!(
        has_visibility,
        "one edit must inject a `private ` visibility prefix on the ctor param: {edits:?}"
    );
    // At least two whole-line deletions: the property decl and the assignment.
    let deletions = edits
        .iter()
        .filter(|e| e["newText"].as_str() == Some(""))
        .count();
    assert!(
        deletions >= 2,
        "must delete both the property decl and the `$this->x = $x;` assignment: {edits:?}"
    );
}
