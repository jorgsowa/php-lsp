#![allow(dead_code)]

use serde_json::{Value, json};
use tower_lsp::lsp_types::Url;

use tokio::io::AsyncWriteExt;

use super::client::{TestClient, frame, read_msg, spawn_server};
use super::fixture::{self, Cursor, Fixture, Range as FixtureRange};
use super::render::{
    assert_highlights_match, assert_locations_match, canonicalize_workspace_edit,
    collect_navigation_annotations, render_call_hierarchy, render_code_actions, render_code_lens,
    render_completion, render_completion_ordered, render_document_symbols, render_folding_ranges,
    render_hover, render_inlay_hints, render_inline_value, render_linked_editing_range,
    render_locations, render_moniker, render_prepare_call_hierarchy, render_prepare_rename,
    render_selection_range, render_semantic_tokens, render_signature_help, render_type_hierarchy,
    render_workspace_symbols,
};

/// Validate that all spans in an LSP response point to valid code symbols.
/// Only validates spans for files present in the fixture (skips cross-file responses).
fn validate_lsp_spans(resp: &Value, _file_path: &str, fixture: &Fixture) {
    let result = &resp["result"];
    let locs: Vec<Value> = if result.is_array() {
        result.as_array().cloned().unwrap_or_default()
    } else if result.is_null() {
        return; // No results to validate
    } else {
        vec![result.clone()]
    };

    for loc in locs {
        let uri = loc["uri"].as_str().or_else(|| loc["targetUri"].as_str());
        let range = if loc["range"].is_object() {
            &loc["range"]
        } else if loc["targetRange"].is_object() {
            &loc["targetRange"]
        } else {
            continue;
        };

        // Extract just the filename from the URI
        let file_path = uri.and_then(|u| u.split('/').last()).unwrap_or("unknown");

        // Find the file in the fixture
        let source = match fixture.files.iter().find(|f| f.path == file_path) {
            Some(f) => f.text.as_str(),
            None => continue, // Skip validation for files not in the fixture
        };

        let start_line = range["start"]["line"].as_u64().unwrap_or(0) as u32;
        let start_char = range["start"]["character"].as_u64().unwrap_or(0) as u32;
        let end_line = range["end"]["line"].as_u64().unwrap_or(0) as u32;
        let end_char = range["end"]["character"].as_u64().unwrap_or(0) as u32;

        // Validate the span points to a valid symbol
        let lines: Vec<&str> = source.lines().collect();
        if start_line as usize >= lines.len() {
            panic!(
                "LSP span exceeds source bounds: start_line {} >= {} lines\nfile: {}\nresponse: {}",
                start_line,
                lines.len(),
                file_path,
                loc
            );
        }

        if start_line == end_line {
            let line = lines[start_line as usize];
            let start = start_char as usize;
            let end = end_char as usize;

            if end > line.len() {
                panic!(
                    "LSP span exceeds line bounds: [{}-{}] exceeds line length {}\n\
                     file: {}, line: {}\n\
                     code: {}\n\
                     response: {}",
                    start,
                    end,
                    line.len(),
                    file_path,
                    start_line,
                    line,
                    loc
                );
            }

            let text = &line[start..end];
            // A leading `\` is valid: fully-qualified names (`\App\Widget`) are
            // legitimate reference spans. A leading `$` is valid for PHP variables.
            // A leading `&` is valid for by-reference use-clause bindings (`use (&$x)`).
            if !text.chars().next().map_or(false, |c| {
                c.is_alphabetic()
                    || c == '_'
                    || c.is_ascii_digit()
                    || c == '\\'
                    || c == '$'
                    || c == '&'
            }) {
                panic!(
                    "LSP span points to invalid symbol start\n\
                     file: {}, line: {}\n\
                     span [{}:{}]: {:?}\n\
                     code: {}\n\
                     response: {}",
                    file_path, start_line, start, end, text, line, loc
                );
            }
        }
    }
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
        // Symlinks in fixtures would be unusual — ignore silently.
    }
    Ok(())
}

/// Convert a `textDocument/rename` response (WorkspaceEdit) into a fake
/// Location-array response so `assert_annotated_locations` can be reused.
///
/// Edits are sorted by (uri, line, character) so annotation matching is
/// deterministic regardless of insertion order.
fn workspace_edit_as_location_response(resp: &Value, root: &str) -> Value {
    let changes = match resp["result"]["changes"].as_object() {
        Some(m) => m,
        None => return json!({"result": []}),
    };
    let mut locs: Vec<Value> = Vec::new();
    for (uri, edits) in changes {
        // Strip the root prefix so the URI matches what fixture paths produce.
        let short_uri = uri
            .strip_prefix(root)
            .unwrap_or(uri.as_str())
            .trim_start_matches('/');
        for edit in edits.as_array().unwrap_or(&vec![]) {
            locs.push(json!({ "uri": short_uri, "range": edit["range"] }));
        }
    }
    locs.sort_by_key(|l| {
        (
            l["uri"].as_str().unwrap_or("").to_owned(),
            l["range"]["start"]["line"].as_u64().unwrap_or(0),
            l["range"]["start"]["character"].as_u64().unwrap_or(0),
        )
    });
    // Reconstruct full URIs so assert_locations_match can compare them.
    // `root` already ends with '/' (e.g. "file:///") and `short` has no
    // leading slash, so simple concatenation produces the correct URI.
    let root_prefix = if root.ends_with('/') {
        root.to_owned()
    } else {
        format!("{root}/")
    };
    let locs: Vec<Value> = locs
        .into_iter()
        .map(|l| {
            let short = l["uri"].as_str().unwrap_or("").to_owned();
            let full_uri = format!("{root_prefix}{short}");
            json!({ "uri": full_uri, "range": l["range"] })
        })
        .collect();
    json!({"result": locs})
}

// ---------- fluent builder ----------

/// High-level test harness. Wraps `TestClient` and handles the boring
/// parts: initialize handshake, didOpen + wait-for-diagnostics, URI building
/// from short paths.
///
/// Each method goes over the wire — there are no internal shortcuts. Drop to
/// `.client()` for escape-hatch access when a test needs custom sequencing.
pub struct TestServer {
    client: TestClient,
    root: Option<std::path::PathBuf>,
    /// Kept alive for the life of the server so the fixture copy isn't
    /// reaped mid-test. `None` when the test provided its own root.
    _fixture_dir: Option<tempfile::TempDir>,
    /// Whether to validate PHP syntax in test fixtures using `php -l`.
    /// Disabled by default. Enable with `validate_syntax(true)` for tests requiring valid PHP.
    validate_syntax: bool,
}

impl TestServer {
    /// Start a server with no workspace root. Use for single-file tests that
    /// don't need PSR-4 autoload or workspace scan.
    pub async fn new() -> Self {
        let mut client = spawn_server();
        Self::do_initialize(&mut client, None).await;
        TestServer {
            client,
            root: None,
            _fixture_dir: None,
            validate_syntax: true,
        }
    }

    /// Start a server rooted at `root`. Does NOT wait for the workspace
    /// index to finish — call `.wait_for_index_ready()` when the test needs
    /// the codebase fast path.
    pub async fn with_root(root: impl AsRef<std::path::Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let mut client = spawn_server();
        Self::do_initialize(&mut client, Some(&root)).await;
        TestServer {
            client,
            root: Some(root),
            _fixture_dir: None,
            validate_syntax: true,
        }
    }

    /// Copy `tests/fixtures/<name>` into a fresh `TempDir` and start a server
    /// rooted there. Each test gets its own isolated copy so mutating
    /// operations (rename, code actions, etc.) don't contaminate siblings.
    ///
    /// The `TempDir` is dropped with the `TestServer`, so callers must keep
    /// the server alive for the duration of the test.
    pub async fn with_fixture(name: &str) -> Self {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = manifest_dir.join("tests/fixtures").join(name);
        assert!(
            source.is_dir(),
            "fixture {name} not found at {} — did you run the fixture acquisition script?",
            source.display()
        );
        let tmp = tempfile::tempdir().expect("create TempDir");
        copy_dir_recursive(&source, tmp.path()).expect("copy fixture");
        let root = tmp.path().to_path_buf();
        let mut client = spawn_server();
        Self::do_initialize(&mut client, Some(&root)).await;
        TestServer {
            client,
            root: Some(root),
            _fixture_dir: Some(tmp),
            validate_syntax: true,
        }
    }

    async fn do_initialize(client: &mut TestClient, root: Option<&std::path::Path>) {
        Self::do_initialize_with(client, root, json!({ "diagnostics": { "enabled": true } })).await;
    }

    async fn do_initialize_with(
        client: &mut TestClient,
        root: Option<&std::path::Path>,
        initialization_options: Value,
    ) -> Value {
        let root_uri = root.map(|p| Url::from_file_path(p).unwrap());
        let root_val = root_uri
            .as_ref()
            .map(|u| json!(u.as_str()))
            .unwrap_or(json!(null));
        let resp = client
            .request(
                "initialize",
                json!({
                    "processId": null,
                    "rootUri": root_val,
                    "capabilities": {
                        "textDocument": {
                            "hover": { "contentFormat": ["markdown", "plaintext"] },
                            "completion": { "completionItem": { "snippetSupport": true } }
                        }
                    },
                    "initializationOptions": initialization_options,
                }),
            )
            .await;
        client.notify("initialized", json!({})).await;
        resp
    }

    /// Start a rootless server with custom `initializationOptions` and return
    /// the raw `initialize` response alongside the server. Use this when a
    /// test needs to inspect `result.capabilities` directly.
    pub async fn new_with_options(initialization_options: Value) -> (Self, Value) {
        let mut client = spawn_server();
        let resp = Self::do_initialize_with(&mut client, None, initialization_options).await;
        let server = TestServer {
            client,
            root: None,
            _fixture_dir: None,
            validate_syntax: true,
        };
        (server, resp)
    }

    /// Variant of `with_fixture` that excludes `vendor/` from the workspace
    /// scan. Use for tests whose subject is workspace code only — `vendor/`
    /// dwarfs the workspace in real-world fixtures (40 MB / 5k+ files for
    /// symfony/demo) and indexing it dominates wall-clock latency.
    pub async fn with_fixture_no_vendor(name: &str) -> Self {
        Self::with_fixture_and_options(name, json!({ "excludePaths": ["vendor/"] })).await
    }

    /// Like `with_fixture`, but pass custom `initializationOptions`. Copies
    /// `tests/fixtures/<name>` into a TempDir so the server has an isolated
    /// workspace, and wires those options into the initialize handshake.
    pub async fn with_fixture_and_options(name: &str, initialization_options: Value) -> Self {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source = manifest_dir.join("tests/fixtures").join(name);
        assert!(
            source.is_dir(),
            "fixture {name} not found at {}",
            source.display()
        );
        let tmp = tempfile::tempdir().expect("create TempDir");
        copy_dir_recursive(&source, tmp.path()).expect("copy fixture");
        let root = tmp.path().to_path_buf();
        let mut client = spawn_server();
        Self::do_initialize_with(&mut client, Some(&root), initialization_options).await;
        TestServer {
            client,
            root: Some(root),
            _fixture_dir: Some(tmp),
            validate_syntax: true,
        }
    }

    /// Start a server rooted at `root` with custom `initializationOptions`.
    /// Used for tests that need to exercise configuration flags
    /// (`phpVersion`, `excludePaths`, etc.) rather than the defaults.
    pub async fn with_root_and_options(
        root: impl AsRef<std::path::Path>,
        initialization_options: Value,
    ) -> Self {
        let root = root.as_ref().to_path_buf();
        let mut client = spawn_server();
        Self::do_initialize_with(&mut client, Some(&root), initialization_options).await;
        TestServer {
            client,
            root: Some(root),
            _fixture_dir: None,
            validate_syntax: true,
        }
    }

    /// Escape hatch for scenarios the builder doesn't cover.
    pub fn client(&mut self) -> &mut TestClient {
        &mut self.client
    }

    /// Build a `file://` URI from a short path. If the server has a root, the
    /// path is resolved relative to it; otherwise it's anchored at a synthetic
    /// absolute URI (e.g. `"a.php"` → `"file:///a.php"`).
    pub fn uri(&self, path: &str) -> String {
        if let Some(root) = &self.root {
            let full = root.join(path);
            Url::from_file_path(full).unwrap().to_string()
        } else {
            // Do NOT use Url::from_file_path here — it rejects paths like
            // "/a.php" on Windows (no drive letter) and panics on unwrap().
            format!("file:///{path}")
        }
    }

    /// Open a document and wait for the first `publishDiagnostics`. This
    /// replaces the `sleep(150ms)` debounce wait in legacy tests — when this
    /// future resolves, parse + semantic analysis have completed.
    ///
    /// Returns the `publishDiagnostics` notification. Tests that want to
    /// inspect diagnostics read from the returned value; chain-style tests
    /// ignore it.
    pub async fn open(&mut self, path: &str, text: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .notify(
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "php",
                        "version": 1,
                        "text": text,
                    }
                }),
            )
            .await;
        self.client.wait_for_diagnostics(&uri).await
    }

    /// Send a full-text `didChange` and wait for the resulting
    /// `publishDiagnostics` — deterministic replacement for the 100 ms
    /// debounce + sleep dance.
    pub async fn change(&mut self, path: &str, version: i32, text: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": text }],
                }),
            )
            .await;
        self.client.wait_for_diagnostics(&uri).await
    }

    /// Send an incremental `didChange` with ranged content changes and wait
    /// for the resulting `publishDiagnostics`. Each entry is
    /// `(start_line, start_char, end_line, end_char, new_text)`; changes are
    /// applied in order, each against the result of the previous one.
    pub async fn change_incremental(
        &mut self,
        path: &str,
        version: i32,
        changes: &[(u32, u32, u32, u32, &str)],
    ) -> Value {
        let uri = self.uri(path);
        let content_changes: Vec<Value> = changes
            .iter()
            .map(|(sl, sc, el, ec, text)| {
                json!({
                    "range": {
                        "start": { "line": sl, "character": sc },
                        "end": { "line": el, "character": ec },
                    },
                    "text": text,
                })
            })
            .collect();
        self.client
            .notify(
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": content_changes,
                }),
            )
            .await;
        self.client.wait_for_diagnostics(&uri).await
    }

    /// Send a request immediately followed by `$/cancelRequest` for its id;
    /// returns the response (see [`TestClient::request_then_cancel`]).
    pub async fn request_then_cancel(&mut self, method: &str, params: serde_json::Value) -> Value {
        self.client.request_then_cancel(method, params).await
    }

    pub async fn wait_for_index_ready(&mut self) -> &mut Self {
        self.client.wait_for_index_ready().await;
        self
    }

    /// Like `wait_for_index_ready` but with a caller-supplied timeout in seconds.
    /// Use for large real-world codebases where the 10 s default is too short.
    pub async fn wait_for_index_ready_secs(&mut self, secs: u64) -> &mut Self {
        self.client.wait_for_index_ready_secs(secs).await;
        self
    }

    /// Enable or disable PHP syntax validation in test fixtures.
    /// Disabled by default (fixtures contain edge cases that aren't valid standalone PHP).
    /// When enabled, `open_fixture` and `check_*` methods will validate that all PHP code
    /// in fixtures is syntactically correct using `php -l`.
    pub fn validate_syntax(&mut self, enabled: bool) -> &mut Self {
        self.validate_syntax = enabled;
        self
    }

    /// Convenience: create a new server with PHP syntax validation enabled.
    /// Use for tests that should only use valid, real-world PHP code.
    pub async fn new_validated() -> Self {
        let mut s = Self::new().await;
        s.validate_syntax(true);
        s
    }

    /// Convenience: create a new rooted server with validation enabled.
    pub async fn with_root_validated(root: impl AsRef<std::path::Path>) -> Self {
        let mut s = Self::with_root(root).await;
        s.validate_syntax(true);
        s
    }

    /// Convenience: create a new server from fixture with validation enabled.
    pub async fn with_fixture_validated(name: &str) -> Self {
        let mut s = Self::with_fixture(name).await;
        s.validate_syntax(true);
        s
    }

    pub async fn wait_until_symbol_present(&mut self, query: &str, timeout: std::time::Duration) {
        let start = std::time::Instant::now();
        loop {
            let resp = self.workspace_symbols(query).await;
            if !resp["result"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true)
            {
                return;
            }
            if start.elapsed() > timeout {
                panic!("wait_until_symbol_present timed out looking for '{query}'");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    pub async fn wait_until_symbol_absent(&mut self, query: &str, timeout: std::time::Duration) {
        let start = std::time::Instant::now();
        loop {
            let resp = self.workspace_symbols(query).await;
            // The server returns `Some(Vec::new())` if non-empty, `None` if
            // empty — both serialize to either `[]` or `null`. "Absent" means
            // either an empty array or a null result.
            let result = &resp["result"];
            let absent =
                result.is_null() || result.as_array().map(|a| a.is_empty()).unwrap_or(false);
            if absent {
                return;
            }
            if start.elapsed() > timeout {
                panic!("wait_until_symbol_absent timed out; '{query}' still present");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    // ---------- feature shortcuts ----------

    pub async fn hover(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/hover",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn definition(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn completion(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/completion",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "context": { "triggerKind": 1 },
                }),
            )
            .await
    }

    pub async fn references(
        &mut self,
        path: &str,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "context": { "includeDeclaration": include_declaration },
                }),
            )
            .await
    }

    pub async fn implementation(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/implementation",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn type_definition(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/typeDefinition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn document_symbols(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await
    }

    pub async fn workspace_symbols(&mut self, query: &str) -> Value {
        self.client
            .request("workspace/symbol", json!({ "query": query }))
            .await
    }

    pub async fn prepare_call_hierarchy(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/prepareCallHierarchy",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn incoming_calls(&mut self, item: Value) -> Value {
        self.client
            .request("callHierarchy/incomingCalls", json!({ "item": item }))
            .await
    }

    pub async fn prepare_type_hierarchy(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/prepareTypeHierarchy",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn supertypes(&mut self, item: Value) -> Value {
        self.client
            .request("typeHierarchy/supertypes", json!({ "item": item }))
            .await
    }

    pub async fn subtypes(&mut self, item: Value) -> Value {
        self.client
            .request("typeHierarchy/subtypes", json!({ "item": item }))
            .await
    }

    pub async fn semantic_tokens_full(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/semanticTokens/full",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await
    }

    pub async fn semantic_tokens_range(
        &mut self,
        path: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
    ) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/semanticTokens/range",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": start_line, "character": start_char },
                        "end": { "line": end_line, "character": end_char },
                    },
                }),
            )
            .await
    }

    pub async fn semantic_tokens_full_delta(
        &mut self,
        path: &str,
        previous_result_id: &str,
    ) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/semanticTokens/full/delta",
                json!({
                    "textDocument": { "uri": uri },
                    "previousResultId": previous_result_id,
                }),
            )
            .await
    }

    pub async fn outgoing_calls(&mut self, item: Value) -> Value {
        self.client
            .request("callHierarchy/outgoingCalls", json!({ "item": item }))
            .await
    }

    pub async fn declaration(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/declaration",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn signature_help(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/signatureHelp",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn document_highlight(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/documentHighlight",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn inlay_hints(
        &mut self,
        path: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
    ) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/inlayHint",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": start_line, "character": start_char },
                        "end": { "line": end_line, "character": end_char },
                    },
                }),
            )
            .await
    }

    pub async fn inlay_hint_resolve(&mut self, hint: Value) -> Value {
        self.client.request("inlayHint/resolve", hint).await
    }

    pub async fn workspace_symbol_resolve(&mut self, symbol: Value) -> Value {
        self.client.request("workspaceSymbol/resolve", symbol).await
    }

    pub async fn completion_resolve(&mut self, item: Value) -> Value {
        self.client.request("completionItem/resolve", item).await
    }

    pub async fn code_action_resolve(&mut self, action: Value) -> Value {
        self.client.request("codeAction/resolve", action).await
    }

    pub async fn code_lens_resolve(&mut self, lens: Value) -> Value {
        self.client.request("codeLens/resolve", lens).await
    }

    pub async fn document_link_resolve(&mut self, link: Value) -> Value {
        self.client.request("documentLink/resolve", link).await
    }

    pub async fn rename(&mut self, path: &str, line: u32, character: u32, new_name: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/rename",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "newName": new_name,
                }),
            )
            .await
    }

    pub async fn prepare_rename(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/prepareRename",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn folding_range(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/foldingRange",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await
    }

    pub async fn code_lens(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/codeLens",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await
    }

    pub async fn selection_range(&mut self, path: &str, positions: Vec<(u32, u32)>) -> Value {
        let uri = self.uri(path);
        let positions: Vec<Value> = positions
            .into_iter()
            .map(|(l, c)| json!({ "line": l, "character": c }))
            .collect();
        self.client
            .request(
                "textDocument/selectionRange",
                json!({
                    "textDocument": { "uri": uri },
                    "positions": positions,
                }),
            )
            .await
    }

    pub async fn document_link(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/documentLink",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await
    }

    pub async fn inline_value(
        &mut self,
        path: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
    ) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/inlineValue",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": start_line, "character": start_char },
                        "end": { "line": end_line, "character": end_char },
                    },
                    "context": {
                        "frameId": 0,
                        "stoppedLocation": {
                            "start": { "line": start_line, "character": start_char },
                            "end": { "line": end_line, "character": end_char },
                        },
                    },
                }),
            )
            .await
    }

    pub async fn pull_diagnostics(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/diagnostic",
                json!({ "textDocument": { "uri": uri } }),
            )
            .await
    }

    pub async fn workspace_diagnostic(&mut self) -> Value {
        self.client
            .request("workspace/diagnostic", json!({ "previousResultIds": [] }))
            .await
    }

    pub async fn workspace_diagnostic_with_prev(&mut self, prev: Vec<(String, String)>) -> Value {
        let ids: Vec<Value> = prev
            .into_iter()
            .map(|(uri, value)| json!({ "uri": uri, "value": value }))
            .collect();
        self.client
            .request("workspace/diagnostic", json!({ "previousResultIds": ids }))
            .await
    }

    pub async fn moniker(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/moniker",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn linked_editing_range(&mut self, path: &str, line: u32, character: u32) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/linkedEditingRange",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                }),
            )
            .await
    }

    pub async fn formatting(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/formatting",
                json!({
                    "textDocument": { "uri": uri },
                    "options": { "tabSize": 4, "insertSpaces": true },
                }),
            )
            .await
    }

    pub async fn range_formatting(
        &mut self,
        path: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
    ) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/rangeFormatting",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": start_line, "character": start_char },
                        "end": { "line": end_line, "character": end_char },
                    },
                    "options": { "tabSize": 4, "insertSpaces": true },
                }),
            )
            .await
    }

    pub async fn on_type_formatting(
        &mut self,
        path: &str,
        line: u32,
        character: u32,
        ch: &str,
    ) -> Value {
        self.on_type_formatting_with_options(path, line, character, ch, 4, true)
            .await
    }

    pub async fn on_type_formatting_with_options(
        &mut self,
        path: &str,
        line: u32,
        character: u32,
        ch: &str,
        tab_size: u32,
        insert_spaces: bool,
    ) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/onTypeFormatting",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "ch": ch,
                    "options": { "tabSize": tab_size, "insertSpaces": insert_spaces },
                }),
            )
            .await
    }

    pub async fn code_action(
        &mut self,
        path: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
    ) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/codeAction",
                json!({
                    "textDocument": { "uri": uri },
                    "range": {
                        "start": { "line": start_line, "character": start_char },
                        "end": { "line": end_line, "character": end_char },
                    },
                    "context": { "diagnostics": [] },
                }),
            )
            .await
    }

    /// Convenience: run `textDocument/codeAction` over a `FixtureRange`.
    /// Typical usage with the two-`$0` selection DSL.
    pub async fn code_action_at(&mut self, r: &FixtureRange) -> Value {
        self.code_action(
            &r.path,
            r.start_line,
            r.start_character,
            r.end_line,
            r.end_character,
        )
        .await
    }

    /// Send `textDocument/didClose` notification.
    pub async fn close(&mut self, path: &str) {
        let uri = self.uri(path);
        self.client
            .notify(
                "textDocument/didClose",
                json!({
                    "textDocument": { "uri": uri }
                }),
            )
            .await;
    }

    /// Send `textDocument/didSave` notification and wait for the
    /// publishDiagnostics the server emits in response.
    pub async fn save(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .notify(
                "textDocument/didSave",
                json!({
                    "textDocument": { "uri": uri }
                }),
            )
            .await;
        self.client.wait_for_diagnostics(&uri).await
    }

    /// Send `textDocument/willSave` notification (void, no response).
    /// `reason` is the LSP `TextDocumentSaveReason`: 1=Manual, 2=AfterDelay, 3=FocusOut.
    pub async fn will_save(&mut self, path: &str, reason: u32) {
        let uri = self.uri(path);
        self.client
            .notify(
                "textDocument/willSave",
                json!({
                    "textDocument": { "uri": uri },
                    "reason": reason
                }),
            )
            .await;
    }

    /// Send `textDocument/willSaveWaitUntil` request and return the response.
    pub async fn will_save_wait_until(&mut self, path: &str) -> Value {
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/willSaveWaitUntil",
                json!({
                    "textDocument": { "uri": uri },
                    "reason": 1
                }),
            )
            .await
    }

    pub async fn will_rename_files(&mut self, renames: Vec<(String, String)>) -> Value {
        let files: Vec<Value> = renames
            .into_iter()
            .map(|(old, new)| json!({ "oldUri": old, "newUri": new }))
            .collect();
        self.client
            .request("workspace/willRenameFiles", json!({ "files": files }))
            .await
    }

    pub async fn will_create_files(&mut self, uris: Vec<String>) -> Value {
        let files: Vec<Value> = uris.into_iter().map(|u| json!({ "uri": u })).collect();
        self.client
            .request("workspace/willCreateFiles", json!({ "files": files }))
            .await
    }

    pub async fn will_delete_files(&mut self, uris: Vec<String>) -> Value {
        let files: Vec<Value> = uris.into_iter().map(|u| json!({ "uri": u })).collect();
        self.client
            .request("workspace/willDeleteFiles", json!({ "files": files }))
            .await
    }

    /// Send `workspace/didChangeWatchedFiles`. Each entry is a `(uri, type)`
    /// pair where type is 1=CREATED, 2=CHANGED, 3=DELETED (LSP FileChangeType).
    ///
    /// The handler runs asynchronously and indexes files before calling
    /// `send_refresh_requests`. Use `workspace_symbols` in a polling loop to
    /// confirm the effect has landed.
    /// Send `workspace/didRenameFiles` notification.
    pub async fn did_rename_files(&mut self, renames: Vec<(String, String)>) {
        let files: Vec<Value> = renames
            .into_iter()
            .map(|(old, new)| json!({ "oldUri": old, "newUri": new }))
            .collect();
        self.client
            .notify("workspace/didRenameFiles", json!({ "files": files }))
            .await;
    }

    /// Send `workspace/didCreateFiles` notification.
    pub async fn did_create_files(&mut self, uris: Vec<String>) {
        let files: Vec<Value> = uris.into_iter().map(|u| json!({ "uri": u })).collect();
        self.client
            .notify("workspace/didCreateFiles", json!({ "files": files }))
            .await;
    }

    /// Send `workspace/didDeleteFiles` notification and wait for the
    /// publishDiagnostics the server sends to clear each deleted file.
    pub async fn did_delete_files(&mut self, uris: Vec<String>) -> Vec<Value> {
        let cloned = uris.clone();
        let files: Vec<Value> = uris.into_iter().map(|u| json!({ "uri": u })).collect();
        self.client
            .notify("workspace/didDeleteFiles", json!({ "files": files }))
            .await;
        let mut results = Vec::new();
        for uri in &cloned {
            results.push(self.client.wait_for_diagnostics(uri).await);
        }
        results
    }

    pub async fn add_workspace_folder(&mut self, folder_uri: &str) {
        self.client
            .notify(
                "workspace/didChangeWorkspaceFolders",
                json!({
                    "event": {
                        "added": [{ "uri": folder_uri, "name": folder_uri }],
                        "removed": [],
                    }
                }),
            )
            .await;
    }

    pub async fn remove_workspace_folder(&mut self, folder_uri: &str) {
        self.client
            .notify(
                "workspace/didChangeWorkspaceFolders",
                json!({
                    "event": {
                        "added": [],
                        "removed": [{ "uri": folder_uri, "name": folder_uri }],
                    }
                }),
            )
            .await;
    }

    pub async fn did_change_watched_files(&mut self, changes: Vec<(String, u32)>) {
        let changes_json: Vec<Value> = changes
            .into_iter()
            .map(|(uri, typ)| json!({ "uri": uri, "type": typ }))
            .collect();
        self.client
            .notify(
                "workspace/didChangeWatchedFiles",
                json!({ "changes": changes_json }),
            )
            .await;
    }

    /// Write `content` to `path` relative to the workspace root. Creates
    /// parent directories as needed.
    pub fn write_file(&self, path: &str, content: &str) {
        let full = self.root.as_ref().expect("server has no root").join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create parent dirs");
        }
        std::fs::write(&full, content).expect("write file");
    }

    /// Delete a file at `path` relative to the workspace root. Ignores errors
    /// if the file is already gone.
    pub fn remove_file(&self, path: &str) {
        let full = self.root.as_ref().expect("server has no root").join(path);
        std::fs::remove_file(&full).ok();
    }

    /// Run `workspace/symbol` for `query` and render the result as sorted
    /// `<kind> <name> @ path:line` lines. Paths are relative to the workspace
    /// root so snapshots are tempdir-agnostic.
    pub async fn snapshot_workspace_symbols(&mut self, query: &str) -> String {
        let resp = self.workspace_symbols(query).await;
        super::render::render_workspace_symbols(&resp, &self.uri(""))
    }

    /// Send `workspace/didChangeConfiguration`, wait for the server to pull
    /// config via `workspace/configuration`, reply with `value`, then drain
    /// messages until the `window/logMessage` completion signal arrives.
    /// Returns that logMessage notification.
    pub async fn change_configuration(&mut self, value: Value) -> Value {
        self.client
            .notify(
                "workspace/didChangeConfiguration",
                json!({ "settings": null }),
            )
            .await;

        let (req_id, _) = self
            .client
            .expect_server_request("workspace/configuration")
            .await;
        self.client
            .reply_to_server_request(req_id, json!([value]))
            .await;

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let msg = read_msg(&mut self.client.read).await;
                // Auto-reply to any server→client requests (refresh burst post-bug-fix)
                if msg.get("method").is_some() {
                    if let Some(srv_id) = msg.get("id") {
                        self.client
                            .write
                            .write_all(&frame(&json!({
                                "jsonrpc": "2.0",
                                "id": srv_id,
                                "result": null,
                            })))
                            .await
                            .unwrap();
                    }
                    // Check if this is the completion log message
                    if msg["method"] == json!("window/logMessage")
                        && msg["params"]["message"]
                            .as_str()
                            .unwrap_or("")
                            .starts_with("php-lsp: using PHP ")
                    {
                        return msg;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("timed out waiting for 'php-lsp: using PHP' log after change_configuration")
        })
    }

    pub async fn shutdown(&mut self) -> Value {
        self.client.request_no_params("shutdown").await
    }

    /// Load a fixture file, find the nth (0-based) occurrence of `needle`,
    /// and return the (text, line, character) for the *start* of the match.
    /// Panics if `needle` isn't found `occurrence + 1` times.
    ///
    /// This is the workhorse for tests against the vendored fixture: real
    /// files don't have `$0` cursor markers, so we locate symbols by
    /// substring. Line/char are 0-based (LSP convention).
    pub fn locate(&self, path: &str, needle: &str, occurrence: usize) -> (String, u32, u32) {
        let full = match &self.root {
            Some(r) => r.join(path),
            None => std::path::PathBuf::from("/").join(path),
        };
        let text = std::fs::read_to_string(&full)
            .unwrap_or_else(|e| panic!("read {}: {e}", full.display()));
        let mut pos = 0usize;
        let mut byte_pos = None;
        for _ in 0..=occurrence {
            let idx = text[pos..].find(needle).unwrap_or_else(|| {
                panic!("needle {needle:?} missing occurrence {occurrence} in {path}")
            });
            byte_pos = Some(pos + idx);
            pos += idx + needle.len();
        }
        let byte_pos = byte_pos.unwrap();
        let before = &text[..byte_pos];
        let line = before.bytes().filter(|b| *b == b'\n').count() as u32;
        let character = before.rsplit('\n').next().unwrap_or("").chars().count() as u32;
        (text, line, character)
    }
}

// ---------- fixture integration ----------

/// Handle returned by `TestServer::open_fixture`. Bundles the parsed fixture
/// with the `publishDiagnostics` notification for each opened file, so tests
/// can reach for either the cursor or a specific file's diagnostics.
pub struct OpenedFixture {
    pub fixture: Fixture,
    /// `publishDiagnostics` payload keyed by fixture path.
    pub diagnostics: std::collections::HashMap<String, Value>,
}

impl OpenedFixture {
    pub fn cursor(&self) -> &Cursor {
        self.fixture
            .cursor
            .as_ref()
            .expect("fixture has no $0 cursor marker")
    }

    /// Range delimited by two `$0` markers (selection). Panics if the fixture
    /// doesn't have exactly two markers.
    pub fn range(&self) -> &FixtureRange {
        self.fixture
            .range
            .as_ref()
            .expect("fixture has no $0…$0 range; put two $0 markers to form a selection")
    }

    pub fn diagnostics_for(&self, path: &str) -> &Value {
        self.diagnostics
            .get(path)
            .unwrap_or_else(|| panic!("no diagnostics recorded for {path}"))
    }
}

impl TestServer {
    /// Parse a multi-file fixture string and open every file over the wire.
    /// Waits for one `publishDiagnostics` per file so analysis has settled
    /// by the time this returns.
    ///
    /// If `validate_syntax` is enabled, validates each file's cleaned PHP code
    /// using `php -l` (fixture parsing removes `$0` markers and annotation lines).
    pub async fn open_fixture(&mut self, src: &str) -> OpenedFixture {
        let fx = fixture::parse(src);
        if self.validate_syntax {
            for file in &fx.files {
                if let Err(e) = crate::common::php_syntax::validate(&file.text) {
                    panic!("invalid PHP syntax in fixture file {}:\n{}", file.path, e);
                }
            }
        }
        let mut diagnostics = std::collections::HashMap::new();
        for file in &fx.files {
            let notif = self.open(&file.path, &file.text).await;
            diagnostics.insert(file.path.clone(), notif);
        }
        OpenedFixture {
            fixture: fx,
            diagnostics,
        }
    }

    /// Open `src` and assert its inline `// ^^^` annotations match the
    /// diagnostics the server publishes for each file. Panics with a
    /// side-by-side diff on mismatch.
    pub async fn check_diagnostics(&mut self, src: &str) {
        let opened = self.open_fixture(src).await;
        for file in &opened.fixture.files {
            fixture::assert_diagnostics(opened.diagnostics_for(&file.path), &file.annotations);
        }
    }

    /// Assert that opening `src` produces zero diagnostics.
    pub async fn check_no_diagnostics(&mut self, src: &str) {
        self.check_diagnostics(src).await;
    }

    /// rust-analyzer-style helper: open `src`, run hover at `$0`, and return
    /// a stable string rendering of the response. Pair with
    /// `expect_test::expect!` to snapshot hover content.
    pub async fn check_hover(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.hover(&c.path, c.line, c.character).await;
        render_hover(&resp)
    }

    /// Open `src`, request completion at `$0`, and return a one-line-per-
    /// item rendering (`<kind> <label>`) sorted by `sortText`.
    pub async fn check_completion(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.completion(&c.path, c.line, c.character).await;
        render_completion(&resp)
    }

    /// Completion at `$0` with full ordered snapshot: kind, label, and detail.
    /// Items are sorted by sortText for deterministic, reproducible snapshots.
    /// Use this for comprehensive ordering and completeness assertions.
    pub async fn check_completion_ordered(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.completion(&c.path, c.line, c.character).await;
        render_completion_ordered(&resp)
    }

    /// Go-to-definition at `$0`, rendered as one `path:line:col-line:col` line
    /// per result. URIs stripped of the workspace-root prefix so snapshots
    /// stay tempdir-agnostic.
    pub async fn check_definition(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.definition(&c.path, c.line, c.character).await;
        render_locations(&resp, &self.uri(""))
    }

    /// References at `$0`, rendered one-per-line (includeDeclaration=true).
    pub async fn check_references(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.references(&c.path, c.line, c.character, true).await;
        render_locations(&resp, &self.uri(""))
    }

    /// Document-symbol outline rendered with indentation per `children`.
    /// The fixture's first file is used.
    pub async fn check_document_symbols(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let resp = self.document_symbols(&path).await;
        render_document_symbols(&resp)
    }

    /// Workspace-symbol search rendered as sorted `<kind> <name> @ path:line`
    /// lines.
    pub async fn check_workspace_symbols(&mut self, src: &str, query: &str) -> String {
        let _ = self.open_fixture(src).await;
        let resp = self.workspace_symbols(query).await;
        render_workspace_symbols(&resp, &self.uri(""))
    }

    /// Signature help at `$0`, rendered as `label` + ` @<active>` for the
    /// active parameter index. Falls back to `<no signature>` when empty.
    pub async fn check_signature_help(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.signature_help(&c.path, c.line, c.character).await;
        render_signature_help(&resp)
    }

    /// Inlay hints over the full text of the fixture's first file, rendered
    /// as sorted `line:col <label>` lines.
    pub async fn check_inlay_hints(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let line_count = opened.fixture.files[0].text.lines().count() as u32;
        let resp = self.inlay_hints(&path, 0, 0, line_count + 1, 0).await;
        render_inlay_hints(&resp)
    }

    pub async fn check_declaration(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.declaration(&c.path, c.line, c.character).await;
        render_locations(&resp, &self.uri(""))
    }

    pub async fn check_type_definition(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.type_definition(&c.path, c.line, c.character).await;
        render_locations(&resp, &self.uri(""))
    }

    pub async fn check_implementation(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.implementation(&c.path, c.line, c.character).await;
        render_locations(&resp, &self.uri(""))
    }

    /// Run `textDocument/codeAction` over the fixture's two-`$0` selection
    /// (falls back to a zero-width range at `$0` if only one cursor is set)
    /// and render the action menu as `<kind> <title>` lines sorted by title.
    pub async fn check_code_actions(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let resp = if let Some(r) = opened.fixture.range.clone() {
            self.code_action_at(&r).await
        } else {
            let c = opened.cursor().clone();
            self.code_action(&c.path, c.line, c.character, c.line, c.character)
                .await
        };
        render_code_actions(&resp)
    }

    /// Fire `textDocument/codeAction`, find the action matching `title`, and return the
    /// complete final file content after applying all edits. Snapshot-asserts the full
    /// transformed file, not just the diffs.
    pub async fn check_code_action_apply(&mut self, src: &str, title: &str) -> String {
        let opened = self.open_fixture(src).await;
        let original_content = opened.fixture.files[0].text.clone();

        let resp = if let Some(r) = opened.fixture.range.clone() {
            self.code_action_at(&r).await
        } else {
            let c = opened.cursor().clone();
            self.code_action(&c.path, c.line, c.character, c.line, c.character)
                .await
        };

        let Some(action) = resp["result"]
            .as_array()
            .and_then(|arr| arr.iter().find(|a| a["title"].as_str() == Some(title)))
            .cloned()
        else {
            return format!("<action not found: {title}>");
        };

        let edit = if action["edit"].is_object() {
            action["edit"].clone()
        } else {
            let resolved = self.code_action_resolve(action).await;
            if let Some(err) = resolved.get("error").filter(|e| !e.is_null()) {
                return format!("<resolve error: {err}>");
            }
            resolved["result"]["edit"].clone()
        };

        let Some(changes) = edit.get("changes").and_then(|c| c.as_object()) else {
            return "<no changes in edit>".to_string();
        };

        let Some(text_edits) = changes.values().next().and_then(|e| e.as_array()) else {
            return "<no text edits>".to_string();
        };

        let mut edits: Vec<_> = text_edits
            .iter()
            .filter_map(|edit| {
                let range = edit.get("range")?;
                let start_line = range.get("start")?.get("line")?.as_u64()? as usize;
                let start_char = range.get("start")?.get("character")?.as_u64()? as usize;
                let end_line = range.get("end")?.get("line")?.as_u64()? as usize;
                let end_char = range.get("end")?.get("character")?.as_u64()? as usize;
                let new_text = edit.get("newText")?.as_str()?.to_string();
                Some(((start_line, start_char, end_line, end_char), new_text))
            })
            .collect();

        edits.sort_by(|a, b| (b.0.0, b.0.1).cmp(&(a.0.0, a.0.1)));

        let lines: Vec<&str> = original_content.lines().collect();
        let mut result = original_content.clone();

        for ((start_line, start_char, end_line, end_char), new_text) in edits {
            let mut byte_start = 0;
            for (i, line) in lines.iter().enumerate() {
                if i == start_line {
                    byte_start += start_char;
                    break;
                }
                byte_start += line.len() + 1;
            }

            let mut byte_end = 0;
            for (i, line) in lines.iter().enumerate() {
                if i == end_line {
                    byte_end += end_char;
                    break;
                }
                byte_end += line.len() + 1;
            }

            if byte_end <= result.len() && byte_start <= byte_end {
                result.replace_range(byte_start..byte_end, &new_text);
            }
        }

        result
    }

    pub async fn check_folding(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let resp = self.folding_range(&path).await;
        render_folding_ranges(&resp)
    }

    /// `textDocument/selectionRange` at the `$0` cursor in `src`, rendered as
    /// one innermost → outermost chain. For multi-position requests use the
    /// lower-level `selection_range` API directly.
    pub async fn check_selection_range(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self
            .selection_range(&c.path, vec![(c.line, c.character)])
            .await;
        render_selection_range(&resp)
    }

    /// Multi-position variant: `textDocument/selectionRange` over all
    /// `(line, character)` pairs in `positions`, rendered one chain per
    /// position separated by `---`.
    pub async fn check_selection_range_at(
        &mut self,
        src: &str,
        positions: Vec<(u32, u32)>,
    ) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let resp = self.selection_range(&path, positions).await;
        render_selection_range(&resp)
    }

    /// Variant of `check_inline_value` taking explicit start/end positions.
    /// Use when the snapshot needs to assert column boundaries that the
    /// `$0…$0` fixture-range form can't express (it always defaults to the
    /// full line on the start/end lines).
    pub async fn check_inline_value_at(
        &mut self,
        src: &str,
        start: (u32, u32),
        end: (u32, u32),
    ) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let resp = self
            .inline_value(&path, start.0, start.1, end.0, end.1)
            .await;
        render_inline_value(&resp)
    }

    /// `textDocument/inlineValue` over the fixture's `$0…$0` range (or the
    /// entire first file when no markers are set), rendered as one
    /// `VariableLookup` per line sorted by start position.
    pub async fn check_inline_value(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let (path, sl, sc, el, ec) = if let Some(r) = opened.fixture.range.clone() {
            (
                r.path,
                r.start_line,
                r.start_character,
                r.end_line,
                r.end_character,
            )
        } else {
            let file = &opened.fixture.files[0];
            let line_count = file.text.lines().count() as u32;
            let last_line_len = file.text.lines().last().map(|l| l.len()).unwrap_or(0) as u32;
            (
                file.path.clone(),
                0u32,
                0u32,
                line_count.saturating_sub(1),
                last_line_len,
            )
        };
        let resp = self.inline_value(&path, sl, sc, el, ec).await;
        render_inline_value(&resp)
    }

    /// `textDocument/linkedEditingRange` at the `$0` cursor in `src`,
    /// rendered as one range per line plus the word pattern; `<no linked
    /// editing>` when the response is null/empty.
    pub async fn check_linked_editing_range(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self
            .linked_editing_range(&c.path, c.line, c.character)
            .await;
        render_linked_editing_range(&resp)
    }

    /// `textDocument/moniker` at the `$0` cursor in `src`, rendered as one
    /// moniker per line (`<scheme>:<identifier> kind=… unique=…`) or
    /// `<no moniker>` when the server returns null/empty.
    pub async fn check_moniker(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.moniker(&c.path, c.line, c.character).await;
        render_moniker(&resp)
    }

    pub async fn check_code_lens(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let resp = self.code_lens(&path).await;
        render_code_lens(&resp)
    }

    /// Prepare type hierarchy at `$0`, render the prepared item(s) directly.
    pub async fn check_prepare_type_hierarchy(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self
            .prepare_type_hierarchy(&c.path, c.line, c.character)
            .await;
        render_type_hierarchy(&resp, &self.uri(""))
    }

    /// Prepare type hierarchy at `$0`, request supertypes, rendered sorted.
    pub async fn check_supertypes(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let prep = self
            .prepare_type_hierarchy(&c.path, c.line, c.character)
            .await;
        let Some(item) = prep["result"].get(0).cloned() else {
            return "<no prepared item>".to_owned();
        };
        if !item.is_object() {
            return "<no prepared item>".to_owned();
        }
        let resp = self.supertypes(item).await;
        render_type_hierarchy(&resp, &self.uri(""))
    }

    pub async fn check_subtypes(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let prep = self
            .prepare_type_hierarchy(&c.path, c.line, c.character)
            .await;
        let Some(item) = prep["result"].get(0).cloned() else {
            return "<no prepared item>".to_owned();
        };
        if !item.is_object() {
            return "<no prepared item>".to_owned();
        }
        let resp = self.subtypes(item).await;
        render_type_hierarchy(&resp, &self.uri(""))
    }

    /// Rename at `$0` with `new_name`, rendered via `canonicalize_workspace_edit`.
    pub async fn check_rename(&mut self, src: &str, new_name: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.rename(&c.path, c.line, c.character, new_name).await;
        if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
            return format!("error: {err}");
        }
        canonicalize_workspace_edit(&resp["result"], &self.uri(""))
    }

    pub async fn check_prepare_rename(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.prepare_rename(&c.path, c.line, c.character).await;
        render_prepare_rename(&resp)
    }

    /// Shared implementation for all annotation-based navigation assertions.
    ///
    /// Validates that every LSP `Location` in `resp` aligns with a `// ^^^ <tag>`
    /// annotation in the fixture, and that no annotation is uncovered.
    fn assert_annotated_locations(
        &self,
        resp: &serde_json::Value,
        fixture: &super::fixture::Fixture,
        cursor_path: &str,
        accept_tags: &[&str],
        label: &str,
    ) {
        validate_lsp_spans(resp, cursor_path, fixture);
        let expected = collect_navigation_annotations(fixture, accept_tags);
        assert_locations_match(resp, &expected, &self.uri(""), label);
    }

    /// Assert that references at `$0` exactly match the `// ^^^ def` and
    /// `// ^^^ ref` annotations in the fixture. Includes declaration in the
    /// request (annotations cover both the decl site and each usage).
    ///
    /// Each LSP `Location` must align with one annotation's range in the file
    /// it lives in; extra or missing locations cause a side-by-side diff.
    pub async fn check_references_annotated(&mut self, src: &str) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.references(&c.path, c.line, c.character, true).await;
        self.assert_annotated_locations(
            &resp,
            &opened.fixture,
            &c.path,
            &["def", "ref"],
            "references",
        );
    }

    /// Assert that go-to-definition at `$0` lands on every `// ^^^ def`
    /// annotation in the fixture.
    pub async fn check_definition_annotated(&mut self, src: &str) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.definition(&c.path, c.line, c.character).await;
        self.assert_annotated_locations(&resp, &opened.fixture, &c.path, &["def"], "definition");
    }

    /// Assert that go-to-declaration at `$0` lands on every `// ^^^ decl`
    /// annotation in the fixture.
    pub async fn check_declaration_annotated(&mut self, src: &str) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.declaration(&c.path, c.line, c.character).await;
        self.assert_annotated_locations(&resp, &opened.fixture, &c.path, &["decl"], "declaration");
    }

    /// Assert that go-to-type-definition at `$0` lands on every `// ^^^ type`
    /// annotation in the fixture.
    pub async fn check_type_definition_annotated(&mut self, src: &str) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.type_definition(&c.path, c.line, c.character).await;
        self.assert_annotated_locations(
            &resp,
            &opened.fixture,
            &c.path,
            &["type"],
            "type_definition",
        );
    }

    /// Assert that go-to-implementation at `$0` lands on every `// ^^^ impl`
    /// annotation in the fixture.
    pub async fn check_implementation_annotated(&mut self, src: &str) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.implementation(&c.path, c.line, c.character).await;
        self.assert_annotated_locations(
            &resp,
            &opened.fixture,
            &c.path,
            &["impl"],
            "implementation",
        );
    }

    /// Open `src`, run hover at `$0`, and assert the rendered output matches
    /// `expected`. Pass `expect![[r#"..."#]]` as the second argument — the
    /// auto-capture feature of `expect_test` still works.
    pub async fn check_hover_annotated(&mut self, src: &str, expected: expect_test::Expect) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.hover(&c.path, c.line, c.character).await;
        expected.assert_eq(&render_hover(&resp));
    }

    /// Assert that rename at `$0` with `new_name` touches exactly the spans
    /// marked with `// ^^^ rename` annotations in the fixture. Each annotation
    /// line covers one rename edit; all edits across all files are matched.
    pub async fn check_rename_annotated(&mut self, src: &str, new_name: &str) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.rename(&c.path, c.line, c.character, new_name).await;
        if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
            panic!("rename errored: {err}");
        }
        let as_locs = workspace_edit_as_location_response(&resp, &self.uri(""));
        self.assert_annotated_locations(&as_locs, &opened.fixture, &c.path, &["rename"], "rename");
    }

    /// Assert that document highlights at `$0` match every `// ^^^ read` /
    /// `// ^^^ write` / `// ^^^ ref` annotation in the same file.
    pub async fn check_highlight_annotated(&mut self, src: &str) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.document_highlight(&c.path, c.line, c.character).await;
        let expected = collect_navigation_annotations(&opened.fixture, &["read", "write", "ref"]);
        // documentHighlight returns ranges without URI; compare by range
        // within the cursor's file only.
        assert_highlights_match(&resp, &expected, &c.path, "document_highlight");
    }

    /// Prepare call hierarchy at `$0` and render the result as
    /// `name (Kind) [detail] @ path:line` (detail omitted when absent).
    pub async fn check_prepare_call_hierarchy(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self
            .prepare_call_hierarchy(&c.path, c.line, c.character)
            .await;
        render_prepare_call_hierarchy(&resp, &self.uri(""))
    }

    /// Prepare call hierarchy at `$0`, request incomingCalls, and render the
    /// callers as sorted `<name> @ path:line` lines.
    pub async fn check_incoming_calls(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let prep = self
            .prepare_call_hierarchy(&c.path, c.line, c.character)
            .await;
        let Some(item) = prep["result"].get(0).cloned() else {
            return "<no prepared item>".to_owned();
        };
        if !item.is_object() {
            return "<no prepared item>".to_owned();
        }
        let resp = self.incoming_calls(item).await;
        render_call_hierarchy(&resp, "from", &self.uri(""))
    }

    /// Prepare call hierarchy at `$0`, request outgoingCalls, and render the
    /// callees as sorted `<name> @ path:line` lines.
    pub async fn check_outgoing_calls(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let prep = self
            .prepare_call_hierarchy(&c.path, c.line, c.character)
            .await;
        let Some(item) = prep["result"].get(0).cloned() else {
            return "<no prepared item>".to_owned();
        };
        if !item.is_object() {
            return "<no prepared item>".to_owned();
        }
        let resp = self.outgoing_calls(item).await;
        render_call_hierarchy(&resp, "to", &self.uri(""))
    }

    /// Request `textDocument/semanticTokens/full` over the first file in the fixture
    /// and render the response using the provided legend types. Returns a stable,
    /// human-readable string suitable for snapshot assertions.
    /// Pass `legend_types` from `init_response["result"]["capabilities"]["semanticTokensProvider"]["legend"]["tokenTypes"]`.
    pub async fn check_semantic_tokens_full(&mut self, src: &str, legend_types: &[&str]) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let resp = self.semantic_tokens_full(&path).await;
        render_semantic_tokens(&resp, legend_types)
    }

    /// Request `textDocument/semanticTokens/range` over the specified range and render
    /// the response using the provided legend types. Returns a stable, human-readable
    /// string suitable for snapshot assertions.
    pub async fn check_semantic_tokens_range(
        &mut self,
        src: &str,
        start_line: u32,
        start_char: u32,
        end_line: u32,
        end_char: u32,
        legend_types: &[&str],
    ) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let resp = self
            .semantic_tokens_range(&path, start_line, start_char, end_line, end_char)
            .await;
        render_semantic_tokens(&resp, legend_types)
    }
}
