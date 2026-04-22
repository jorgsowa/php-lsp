//! Shared E2E test harness: a real LSP server wired over in-memory duplex
//! streams, plus a fluent `TestServer` builder to cut down on JSON-RPC
//! boilerplate in tests.
//!
//! The harness speaks the full LSP wire protocol — no internal API shortcuts —
//! so tests exercise the same path a real editor client would.

#![allow(dead_code)]

use php_lsp::backend::Backend;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tower_lsp::lsp_types::Url;
use tower_lsp::{LspService, Server};

// ---------- low-level framing ----------

fn frame(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_string(msg).unwrap();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

async fn read_msg(reader: &mut (impl AsyncReadExt + Unpin)) -> Value {
    let mut header_buf = Vec::new();
    loop {
        let b = reader.read_u8().await.expect("read byte");
        header_buf.push(b);
        if header_buf.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let header_str = std::str::from_utf8(&header_buf).unwrap();
    let content_length: usize = header_str
        .lines()
        .find(|l| l.to_lowercase().starts_with("content-length:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse().ok())
        .expect("Content-Length header");
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).await.expect("read body");
    serde_json::from_slice(&body).expect("parse JSON")
}

// ---------- raw client ----------

/// Minimal LSP client over in-memory duplex streams. Prefer `TestServer` for
/// feature tests — drop to `TestClient` only when a scenario needs unusual
/// message sequencing.
pub struct TestClient {
    write: WriteHalf<DuplexStream>,
    read: ReadHalf<DuplexStream>,
    next_id: u64,
}

impl TestClient {
    pub async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write.write_all(&frame(&msg)).await.unwrap();
        loop {
            let resp = read_msg(&mut self.read).await;
            if resp.get("id") == Some(&json!(id)) {
                return resp;
            }
            // notifications (publishDiagnostics, logMessage, …) — skip
        }
    }

    pub async fn request_no_params(&mut self, method: &str) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });
        self.write.write_all(&frame(&msg)).await.unwrap();
        loop {
            let resp = read_msg(&mut self.read).await;
            if resp.get("id") == Some(&json!(id)) {
                return resp;
            }
        }
    }

    pub async fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write.write_all(&frame(&msg)).await.unwrap();
    }

    /// Block until a notification with `method` arrives. 5 s timeout.
    pub async fn read_notification(&mut self, method: &str) -> Value {
        tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method") == Some(&json!(method)) {
                    return msg;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {method} notification"))
    }

    /// Block until `textDocument/publishDiagnostics` arrives for `uri`.
    /// Since `did_open` publishes diagnostics synchronously after parse +
    /// semantic analysis finish, this is a deterministic replacement for
    /// `sleep(150ms)` debounce waits.
    pub async fn wait_for_diagnostics(&mut self, uri: &str) -> Value {
        let uri_val = json!(uri);
        tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method") == Some(&json!("textDocument/publishDiagnostics"))
                    && msg["params"]["uri"] == uri_val
                {
                    return msg;
                }
                // Server-to-client request (e.g. WorkDoneProgressCreate during
                // workspace scan): reply null so the server isn't blocked.
                if msg.get("method").is_some() {
                    if let Some(id) = msg.get("id") {
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": null,
                        });
                        self.write.write_all(&frame(&response)).await.unwrap();
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for publishDiagnostics for {uri}"))
    }

    /// Wait for `$/php-lsp/indexReady` (10 s timeout). Auto-replies to any
    /// server-to-client requests sent during the workspace scan.
    pub async fn wait_for_index_ready(&mut self) {
        tokio::time::timeout(tokio::time::Duration::from_secs(10), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method") == Some(&json!("$/php-lsp/indexReady")) {
                    return;
                }
                if msg.get("method").is_some() {
                    if let Some(id) = msg.get("id") {
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": null,
                        });
                        self.write.write_all(&frame(&response)).await.unwrap();
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for $/php-lsp/indexReady"))
    }
}

fn spawn_server() -> TestClient {
    let (client_stream, server_stream) = tokio::io::duplex(1 << 20);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (service, socket) = LspService::new(Backend::new);
    tokio::spawn(Server::new(server_read, server_write, socket).serve(service));
    TestClient {
        write: client_write,
        read: client_read,
        next_id: 1,
    }
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

/// Recursively copy a directory tree. Minimal — no symlink handling, no
/// permissions preservation; fine for our test fixtures.
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
                    "initializationOptions": { "diagnostics": { "enabled": true } }
                }),
            )
            .await;
        client.notify("initialized", json!({})).await;
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

// ---------- cursor-marker helper ----------

/// Extract a `$0` cursor marker from `src`, returning the cleaned source and
/// the 0-based (line, character) of where the marker was. Mirrors
/// `src/test_utils::cursor` (not importable from `tests/`).
pub fn cursor(src: &str) -> (String, u32, u32) {
    let idx = src.find("$0").expect("missing $0 cursor marker");
    let before = &src[..idx];
    let line = before.bytes().filter(|b| *b == b'\n').count() as u32;
    let character = before.rsplit('\n').next().unwrap_or("").chars().count() as u32;
    let cleaned = format!("{}{}", &src[..idx], &src[idx + 2..]);
    (cleaned, line, character)
}
