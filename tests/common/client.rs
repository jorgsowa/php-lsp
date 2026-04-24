#![allow(dead_code)]

use php_lsp::backend::Backend;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
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
    pub(crate) write: WriteHalf<DuplexStream>,
    pub(crate) read: ReadHalf<DuplexStream>,
    pub(crate) next_id: u64,
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

pub(crate) fn spawn_server() -> TestClient {
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
