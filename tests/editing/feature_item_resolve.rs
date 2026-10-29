//! Resolve provider tests for all LSP item types.
//! Tests verify that lazy-loaded item data (detail, documentation, edit computation)
//! is correctly resolved when requested via the LSP resolve protocol.

use super::*;
use expect_test::expect;
use serde_json::json;

// ============================================================================
// completionItem/resolve tests
// ============================================================================

#[tokio::test]
async fn completion_resolve_adds_documentation_to_function() {
    let mut s = TestServer::new().await;
    s.open("file.php", "<?php\narray_map$0").await;

    let resp = s.completion("file.php", 1, 9).await;
    let items: Vec<_> = resp["result"]
        .as_array()
        .or_else(|| resp["result"]["items"].as_array())
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let item = items.iter().find(|i| {
        i["label"]
            .as_str()
            .map(|l| l == "array_map")
            .unwrap_or(false)
    });
    assert!(item.is_some(), "array_map not found");

    let resolved = s.completion_resolve(item.unwrap().clone()).await;
    let out = render_resolved_completion_item(&resolved);
    expect![[r#"
        array_map (Function)
        detail: function array_map(callable|null $callback, array $array, array ...$arrays): array
        docs: Applies the callback to the elements of the given arrays

        [php.net documentation](https://www.php.net/function.array-map)"#]]
    .assert_eq(&out);
}

#[tokio::test]
async fn completion_resolve_idempotent_when_resolved() {
    let mut s = TestServer::new().await;
    s.open("file.php", "<?php\nstrlen").await;

    let resp = s.completion("file.php", 1, 6).await;
    let items: Vec<_> = resp["result"]
        .as_array()
        .or_else(|| resp["result"]["items"].as_array())
        .map(|a| a.to_vec())
        .unwrap_or_default();

    assert!(
        !items.is_empty(),
        "completions must be available at position 1:6"
    );
    let item = items[0].clone();
    let resolved1 = s.completion_resolve(item).await;
    let resolved2 = s.completion_resolve(resolved1["result"].clone()).await;

    let out1 = render_resolved_completion_item(&resolved1);
    let out2 = render_resolved_completion_item(&resolved2);
    assert_eq!(out1, out2, "resolve should be idempotent");
    expect![[r#"
        strlen (Function)
        detail: function strlen(string $string): int<0, max>
        docs: Get string length

        [php.net documentation](https://www.php.net/function.strlen)"#]]
    .assert_eq(&out1);
}

/// Regression test for a confirmed bug: two unrelated classes declaring a
/// same-named method both live in the workspace index; completion_resolve
/// looked up detail/documentation by bare method name across the whole
/// workspace with no notion of which class the completion item actually
/// belonged to, so it could return the wrong class's docblock/signature.
#[tokio::test]
async fn completion_resolve_scopes_to_owning_class_on_name_collision() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("Wanted.php"),
        "<?php\nnamespace App\\Wanted;\nclass Target {\n    /** The one we want. */\n    public static function save(): string { return 'w'; }\n}\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("Other.php"),
        "<?php\nnamespace App\\Other;\nclass Target {\n    /** Not this one. */\n    public static function save(int $x): void {}\n}\n",
    )
    .unwrap();
    let caller = "<?php\nuse App\\Wanted\\Target;\n\nfunction f() {\n    Target::save();\n}\n";
    std::fs::write(tmp.path().join("caller.php"), caller).unwrap();

    let mut s = TestServer::with_root(tmp.path()).await;
    s.validate_syntax(false);
    s.wait_for_index_ready().await;
    s.open("caller.php", caller).await;

    let (_, line, ch) = s.locate("caller.php", "save();", 0);
    let resp = s.completion("caller.php", line, ch).await;
    let items: Vec<_> = resp["result"]
        .as_array()
        .or_else(|| resp["result"]["items"].as_array())
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("save"))
        .expect("save not in completions")
        .clone();

    let resolved = s.completion_resolve(item).await;
    let out = render_resolved_completion_item(&resolved);
    assert!(
        out.contains("The one we want."),
        "resolve must use the imported Target::save's own docblock, got: {out}"
    );
    assert!(
        !out.contains("Not this one."),
        "resolve must not pick up the unrelated same-named method's docblock, got: {out}"
    );
}

#[tokio::test]
async fn completion_resolve_fills_open_doc_namespaced_function() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);
    let src = "<?php\nnamespace App\\Util;\n/** Says hello. */\nfunction greetUser(string $name): string { return $name; }\ngreetU";
    s.open("file.php", src).await;
    let resp = s.completion("file.php", 4, 6).await;
    let items: Vec<_> = resp["result"]
        .as_array()
        .or_else(|| resp["result"]["items"].as_array())
        .map(|a| a.to_vec())
        .unwrap_or_default();
    let item = items
        .iter()
        .find(|i| i["label"].as_str() == Some("greetUser"))
        .expect("greetUser not in completions")
        .clone();
    assert_eq!(item["data"]["fqn"], "App\\Util\\greetUser");

    let resolved = s.completion_resolve(item).await;
    let out = render_resolved_completion_item(&resolved);
    assert!(
        out.contains("detail: function greetUser(string $name): string"),
        "got: {out}"
    );
    assert!(out.contains("Says hello."), "got: {out}");
}

// ============================================================================
// codeAction/resolve tests
// ============================================================================

#[tokio::test]
async fn code_action_resolve_defers_extract_method_edit() {
    let mut s = TestServer::new().await;
    let src = r#"<?php
class Math {
    public function add(): int {
        return $01 + 2$0;
    }
}
"#;
    let opened = s.open_fixture(src).await;
    let resp = if let Some(r) = opened.fixture.range.clone() {
        s.code_action_at(&r).await
    } else {
        let c = opened.cursor().clone();
        s.code_action(&c.path, c.line, c.character, c.line, c.character)
            .await
    };
    let actions: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let extract_action = actions.iter().find(|a| {
        a["title"]
            .as_str()
            .map(|t| t.contains("Extract method"))
            .unwrap_or(false)
    });

    if let Some(action) = extract_action {
        // CRITICAL: Verify deferred behavior - action must have data, NO edit before resolve
        assert!(
            action["edit"].is_null(),
            "deferred action should NOT have edit before resolve"
        );
        assert!(
            action["data"].is_object(),
            "deferred action must have data with resolve metadata"
        );
        assert_eq!(
            action["data"]["php_lsp_resolve"].as_str(),
            Some("extract_method"),
            "data must have php_lsp_resolve tag"
        );

        let resolved = s.code_action_resolve(action.clone()).await;
        let out = render_resolved_code_action(&resolved, &s.uri(""));

        // Verify edit was computed during resolve
        expect![[r#"
Extract method (RefactorExtract)
edit: 1 file(s) modified"#]]
        .assert_eq(&out);
    }
}

#[tokio::test]
async fn code_action_resolve_handles_non_deferred_actions() {
    let mut s = TestServer::new().await;
    let src = r#"<?php
function $0greet() {
    echo "Hello";
}"#;

    let opened = s.open_fixture(src).await;
    let resp = if let Some(r) = opened.fixture.range.clone() {
        s.code_action_at(&r).await
    } else {
        let c = opened.cursor().clone();
        s.code_action(&c.path, c.line, c.character, c.line, c.character)
            .await
    };
    let actions: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let mut snapshots: Vec<String> = Vec::new();
    for action in actions {
        let resolved = s.code_action_resolve(action.clone()).await;
        assert!(
            resolved["error"].is_null(),
            "all actions should resolve successfully"
        );
        snapshots.push(render_resolved_code_action(&resolved, &s.uri("")));
    }
    expect![[r#"
        Generate PHPDoc (refactor)
        edit: 1 file(s) modified
        ---
        Add return type `: void` (refactor)
        edit: 1 file(s) modified"#]]
    .assert_eq(&snapshots.join("\n---\n"));
}

// ============================================================================
// codeLens/resolve tests
// ============================================================================

#[tokio::test]
async fn code_lens_resolve_returns_populated_lens() {
    let mut s = TestServer::new().await;
    let src = r#"<?php
class TestCase {
    public function testExample(): void {}

    public function runIt(): void {
        $this->testExample();
    }
}"#;

    s.open("test.php", src).await;

    let resp = s.code_lens("test.php").await;
    let lenses: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let mut results = Vec::new();
    for lens in lenses {
        let resolved = s.code_lens_resolve(lens.clone()).await;
        results.push(render_resolved_code_lens(&resolved));
    }

    expect![[r#"L1:6 0 references [editor.action.showReferences]
L2:20 1 reference [editor.action.showReferences]
L2:20 ▶ Run test [php-lsp.runTest]
L4:20 0 references [editor.action.showReferences]"#]]
    .assert_eq(&results.join("\n"));
}

#[tokio::test]
async fn code_lens_resolve_preserves_position() {
    let mut s = TestServer::new().await;
    let src = r#"<?php
class Service {
    public function execute(): void {}
}
"#;

    s.open("service.php", src).await;

    let resp = s.code_lens("service.php").await;
    let lenses: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let mut results = Vec::new();
    for lens in lenses {
        let range_before = lens["range"].clone();
        let resolved = s.code_lens_resolve(lens.clone()).await;
        let range_after = resolved["result"]["range"].clone();

        assert_eq!(range_before, range_after, "resolve should not modify range");
        results.push(render_resolved_code_lens(&resolved));
    }
    expect![[r#"
        L1:6 0 references [editor.action.showReferences]
        L2:20 0 references [editor.action.showReferences]"#]]
    .assert_eq(&results.join("\n"));
}

// ============================================================================
// documentLink/resolve tests
// ============================================================================

#[tokio::test]
async fn document_link_resolve_returns_target() {
    let mut s = TestServer::new().await;
    s.open(
        "links.php",
        "<?php\nrequire_once 'vendor/autoload.php';\nrequire 'lib/config.php';\n",
    )
    .await;

    let resp = s.document_link("links.php").await;
    let links: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let mut results = Vec::new();
    for link in links {
        let resolved = s.document_link_resolve(link.clone()).await;
        results.push(render_resolved_document_link(&resolved, &s.uri("")));
    }

    expect![[r#"L1:14 -> vendor/autoload.php
L2:9 -> lib/config.php"#]]
    .assert_eq(&results.join("\n"));
}

#[tokio::test]
async fn document_link_resolve_handles_http_links() {
    let mut s = TestServer::new().await;
    s.open(
        "doc.php",
        "<?php\n/** @link https://php.net/manual */\nfunction helper() {}\n",
    )
    .await;

    let resp = s.document_link("doc.php").await;
    let links: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    for link in links {
        let resolved = s.document_link_resolve(link.clone()).await;
        let out = render_resolved_document_link(&resolved, &s.uri(""));

        expect![[r#"L1:10 -> https://php.net/manual"#]].assert_eq(&out);
    }
}

// ============================================================================
// inlayHint/resolve tests
// ============================================================================

#[tokio::test]
async fn inlay_hint_resolve_adds_tooltip() {
    let mut s = TestServer::new().await;
    let src = r#"<?php
function process(string $name, int $age): void {}
$0process("Alice", 30);$0
"#;
    let opened = s.open_fixture(src).await;
    let path = opened.fixture.files[0].path.clone();

    let resp = if let Some(r) = opened.fixture.range.clone() {
        s.inlay_hints(
            &r.path,
            r.start_line,
            r.start_character,
            r.end_line,
            r.end_character,
        )
        .await
    } else {
        s.inlay_hints(&path, 0, 0, u32::MAX, u32::MAX).await
    };
    let hints: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let mut snapshots: Vec<String> = Vec::new();
    for hint in hints {
        let resolved = s.inlay_hint_resolve(hint.clone()).await;
        assert!(
            resolved["error"].is_null(),
            "hint should resolve without error"
        );
        snapshots.push(render_resolved_inlay_hint(&resolved));
    }
    expect![[r#"
        2:8 name:
        tooltip: ```php
        function process(string $name, int $age): void
        ```
        ---
        2:17 age:
        tooltip: ```php
        function process(string $name, int $age): void
        ```"#]]
    .assert_eq(&snapshots.join("\n---\n"));
}

#[tokio::test]
async fn inlay_hint_resolve_idempotent() {
    let mut s = TestServer::new().await;
    let src = r#"<?php
function getName(string $first, string $last): string { return $first; }
$0getName("Ada", "Lovelace");$0
"#;
    let opened = s.open_fixture(src).await;
    let path = opened.fixture.files[0].path.clone();

    let resp = if let Some(r) = opened.fixture.range.clone() {
        s.inlay_hints(
            &r.path,
            r.start_line,
            r.start_character,
            r.end_line,
            r.end_character,
        )
        .await
    } else {
        s.inlay_hints(&path, 0, 0, u32::MAX, u32::MAX).await
    };
    let hints: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    let mut snapshots = Vec::new();
    for hint in hints {
        let resolved1 = s.inlay_hint_resolve(hint.clone()).await;
        let resolved2 = s.inlay_hint_resolve(resolved1["result"].clone()).await;

        // Resolving twice should be idempotent
        let out1 = render_resolved_inlay_hint(&resolved1);
        let out2 = render_resolved_inlay_hint(&resolved2);
        assert_eq!(out1, out2, "resolve should be idempotent");
        snapshots.push(out1);
    }
    expect![[r#"
        2:8 first:
        tooltip: ```php
        function getName(string $first, string $last): string
        ```
        ---
        2:15 last:
        tooltip: ```php
        function getName(string $first, string $last): string
        ```"#]]
    .assert_eq(&snapshots.join("\n---\n"));
}

// ============================================================================
// workspaceSymbol/resolve tests
// ============================================================================

#[tokio::test]
async fn workspace_symbol_resolve_populates_location() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);

    // Open fixture and search for Database class
    let _ = s
        .open_fixture(
            r#"<?php
class Database {}
"#,
        )
        .await;

    let resp = s.workspace_symbols("Database").await;
    let symbols: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    assert!(!symbols.is_empty(), "Database symbol not found");

    let symbol = symbols[0].clone();
    let resolved = s.workspace_symbol_resolve(symbol).await;
    let out = render_resolved_workspace_symbol(&resolved, &s.uri(""));
    expect!["Database (Class) @ main.php:1:6-1:14"].assert_eq(&out);
}

#[tokio::test]
async fn workspace_symbol_resolve_multiple_symbols() {
    let mut s = TestServer::new().await;
    s.validate_syntax(false);

    let _ = s
        .open_fixture(
            r#"<?php
function test() {}
function testing() {}
function tested() {}
"#,
        )
        .await;

    let resp = s.workspace_symbols("test").await;
    let symbols: Vec<_> = resp["result"]
        .as_array()
        .map(|a| a.to_vec())
        .unwrap_or_default();

    assert!(!symbols.is_empty(), "should find symbols matching 'test'");

    let mut results = Vec::new();
    for symbol in symbols.iter().take(5) {
        let resolved = s.workspace_symbol_resolve(symbol.clone()).await;
        assert_eq!(
            symbol["name"], resolved["result"]["name"],
            "resolve should preserve name"
        );
        assert_eq!(
            symbol["kind"], resolved["result"]["kind"],
            "resolve should preserve kind"
        );
        results.push(render_resolved_workspace_symbol(&resolved, &s.uri("")));
    }
    expect![[r#"
        test (Function) @ main.php:1:9-1:13
        testing (Function) @ main.php:2:9-2:16
        tested (Function) @ main.php:3:9-3:15"#]]
    .assert_eq(&results.join("\n"));
}

// ============================================================================
// Error handling tests
// ============================================================================

#[tokio::test]
async fn resolve_handles_empty_items_gracefully() {
    let mut s = TestServer::new().await;

    let empty_item = json!({});

    let resolved = s.completion_resolve(empty_item).await;
    let out = render_resolved_completion_item(&resolved);

    expect![[r#"error: {"code":-32602,"message":"missing field `label`"}"#]].assert_eq(&out);
}
