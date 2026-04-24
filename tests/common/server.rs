#![allow(dead_code)]

use serde_json::{Value, json};
use tower_lsp::lsp_types::Url;

use super::client::{TestClient, spawn_server};
use super::fixture::{self, Cursor, Fixture, Range as FixtureRange};
use super::render::{
    assert_highlights_match, assert_locations_match, canonicalize_workspace_edit,
    collect_navigation_annotations, render_call_hierarchy, render_code_actions, render_code_lens,
    render_completion, render_document_symbols, render_folding_ranges, render_hover,
    render_inlay_hints, render_locations, render_prepare_rename, render_signature_help,
    render_type_hierarchy, render_workspace_symbols,
};

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

// ---------- fluent builder ----------

/// High-level E2E test harness. Wraps `TestClient` and handles the boring
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
        }
    }

    async fn do_initialize(client: &mut TestClient, root: Option<&std::path::Path>) {
        Self::do_initialize_with(client, root, json!({ "diagnostics": { "enabled": true } })).await;
    }

    async fn do_initialize_with(
        client: &mut TestClient,
        root: Option<&std::path::Path>,
        initialization_options: Value,
    ) {
        let root_uri = root.map(|p| Url::from_file_path(p).unwrap());
        let root_val = root_uri
            .as_ref()
            .map(|u| json!(u.as_str()))
            .unwrap_or(json!(null));
        client
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
        }
    }

    /// Escape hatch for scenarios the builder doesn't cover.
    pub fn client(&mut self) -> &mut TestClient {
        &mut self.client
    }

    /// Build a `file://` URI from a short path. If the server has a root, the
    /// path is resolved relative to it; otherwise it's anchored at `/` so the
    /// resulting URI is still absolute (e.g. `"a.php"` → `"file:///a.php"`).
    pub fn uri(&self, path: &str) -> String {
        if let Some(root) = &self.root {
            let full = root.join(path);
            Url::from_file_path(full).unwrap().to_string()
        } else {
            let full = std::path::Path::new("/").join(path);
            Url::from_file_path(full).unwrap().to_string()
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

    pub async fn wait_for_index_ready(&mut self) -> &mut Self {
        self.client.wait_for_index_ready().await;
        self
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

    pub async fn completion_resolve(&mut self, item: Value) -> Value {
        self.client.request("completionItem/resolve", item).await
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
        let uri = self.uri(path);
        self.client
            .request(
                "textDocument/onTypeFormatting",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": line, "character": character },
                    "ch": ch,
                    "options": { "tabSize": 4, "insertSpaces": true },
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
    pub async fn open_fixture(&mut self, src: &str) -> OpenedFixture {
        let fx = fixture::parse(src);
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

    pub async fn check_folding(&mut self, src: &str) -> String {
        let opened = self.open_fixture(src).await;
        let path = opened.fixture.files[0].path.clone();
        let resp = self.folding_range(&path).await;
        render_folding_ranges(&resp)
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
        let expected = collect_navigation_annotations(&opened.fixture, &["def", "ref"]);
        assert_locations_match(&resp, &expected, &self.uri(""), "references");
    }

    /// Assert that go-to-definition at `$0` lands on every `// ^^^ def`
    /// annotation in the fixture.
    pub async fn check_definition_annotated(&mut self, src: &str) {
        let opened = self.open_fixture(src).await;
        let c = opened.cursor().clone();
        let resp = self.definition(&c.path, c.line, c.character).await;
        let expected = collect_navigation_annotations(&opened.fixture, &["def"]);
        assert_locations_match(&resp, &expected, &self.uri(""), "definition");
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
}
