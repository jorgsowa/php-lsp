#![allow(dead_code)]

use php_lsp::backend::Backend;
use serde_json::{Value, json};
use std::collections::VecDeque;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};
use tokio::time::Duration;
use tower_lsp_server::{LspService, Server};

// ---------- low-level framing ----------

pub(super) fn frame(msg: &Value) -> Vec<u8> {
    let body = serde_json::to_string(msg).unwrap();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

pub(super) async fn read_msg(reader: &mut (impl AsyncReadExt + Unpin)) -> Value {
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

/// Pull the next protocol message, preferring anything an earlier, unrelated
/// wait already read off the wire and rebuffered (via `pending`) because it
/// wasn't what *that* call was looking for — e.g. a `$/php-lsp/indexReady`
/// notification seen while something else was only waiting on a specific
/// `publishDiagnostics`. Without this, that notification would be read once
/// and gone, and a later `wait_for_index_ready()` would hang forever.
///
/// Server→client requests (method + id) are ACKed with `null` inline and
/// never returned — the server may be blocked on that reply regardless of
/// which wait loop happens to observe it, and no caller here needs anything
/// but `null` back.
///
/// `budget` bounds how many already-buffered entries this call will draw
/// down before falling back to a real (yielding) socket read. Without a
/// bound, a call whose target never shows up among the buffered messages
/// would requeue-and-repop the same items forever without ever performing a
/// real `.await` on I/O — starving the `tokio::time::timeout` driving it on
/// a current-thread runtime instead of eventually erroring out.
async fn recv_or_buffered(
    pending: &mut VecDeque<Value>,
    read: &mut ReadHalf<DuplexStream>,
    write: &mut WriteHalf<DuplexStream>,
    budget: &mut usize,
) -> Value {
    loop {
        let msg = if *budget > 0 {
            *budget -= 1;
            pending
                .pop_front()
                .expect("budget bounded by pending.len()")
        } else {
            read_msg(read).await
        };
        if msg.get("method").is_some()
            && let Some(id) = msg.get("id")
        {
            let response = json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": null,
            });
            write.write_all(&frame(&response)).await.unwrap();
            continue;
        }
        return msg;
    }
}

// ---------- raw client ----------

/// Minimal LSP client over in-memory duplex streams. Prefer `TestServer` for
/// feature tests — drop to `TestClient` only when a scenario needs unusual
/// message sequencing.
pub struct TestClient {
    pub(crate) write: WriteHalf<DuplexStream>,
    pub(crate) read: ReadHalf<DuplexStream>,
    pub(crate) next_id: u64,
    /// Notifications/responses read off the wire by one `wait_for_*` call
    /// that didn't match what it was looking for, held here for whichever
    /// later call does want them. See `recv_or_buffered`.
    pending: VecDeque<Value>,
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
        let method_owned = method.to_owned();
        // Safe to search history: `id` is unique per request, so a buffered
        // match can only ever be the one response for this exact call.
        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("id") == Some(&json!(id)) {
                    return msg;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for response to {method_owned}"))
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
        let method_owned = method.to_owned();
        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("id") == Some(&json!(id)) {
                    return msg;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for response to {method_owned}"))
    }

    /// Send a request and immediately follow it with `$/cancelRequest` for
    /// the same id — without waiting in between — then return the response
    /// for that id. Pins tower-lsp's cancellation semantics: a request whose
    /// handler is still pending when the cancel is processed resolves to a
    /// `RequestCancelled` (-32800) error.
    pub async fn request_then_cancel(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write.write_all(&frame(&msg)).await.unwrap();
        let cancel = json!({
            "jsonrpc": "2.0",
            "method": "$/cancelRequest",
            "params": { "id": id },
        });
        self.write.write_all(&frame(&cancel)).await.unwrap();
        let method_owned = method.to_owned();
        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("id") == Some(&json!(id)) {
                    return msg;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for response to {method_owned}"))
    }

    /// Write `method`/`params` as a new request and return its id without
    /// waiting for the response. Pairs with `assert_stays_responsive` below,
    /// which needs two requests in flight at once.
    async fn send_request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write.write_all(&frame(&msg)).await.unwrap();
        id
    }

    /// Regression guard for the class of bug where a handler runs
    /// synchronous, non-yielding work (a full-document walk, a blocking
    /// subprocess, a workspace scan) directly on the async task instead of
    /// via `spawn_blocking`. `tower-lsp-server`'s transport drives
    /// stdin-reading, response-writing, and every in-flight handler as ONE
    /// joined future (`Server::serve`); a handler that never yields
    /// monopolizes that single poll for its whole duration, so nothing
    /// else — not even an unrelated, trivially cheap request — can complete
    /// until it's done.
    ///
    /// Sends `slow_method` then immediately a cheap `$/php-lsp/debugStats`
    /// probe (a handful of atomic loads, no `.await` of its own), without
    /// waiting in between, and asserts the probe's response is the one to
    /// arrive first. This is an ordering check, not a timing threshold: if
    /// `slow_method` blocks the shared task, its own response necessarily
    /// becomes ready before the probe is ever polled; if it correctly
    /// defers to `spawn_blocking`, its poll returns `Pending` almost
    /// instantly and the probe resolves first on that same turn —
    /// deterministic regardless of machine speed, so `slow_params` only
    /// needs to make the handler's work non-trivial, not slow in absolute
    /// terms.
    pub async fn assert_stays_responsive(&mut self, slow_method: &str, slow_params: Value) {
        let slow_id = self.send_request(slow_method, slow_params).await;
        let probe_id = self.send_request("$/php-lsp/debugStats", json!({})).await;

        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        let probe_won = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("id") == Some(&json!(probe_id)) {
                    return true;
                }
                if msg.get("id") == Some(&json!(slow_id)) {
                    return false;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {slow_method}/debugStats responses"));

        assert!(
            probe_won,
            "{slow_method} appears to block the request loop: its own response arrived \
             before the concurrently-sent debugStats probe, meaning it ran synchronous \
             work inline instead of via spawn_blocking"
        );

        // Drain the slow response too so it isn't left unread on the wire.
        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("id") == Some(&json!(slow_id)) {
                    return;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {slow_method} response"));
    }

    /// Notification-flavored counterpart to `assert_stays_responsive`, for
    /// handlers with no response of their own (e.g. `textDocument/didOpen`).
    /// A notification can't be ordered against a probe response the way a
    /// request can, so this falls back to an absolute `budget` around the
    /// whole exchange: send `notif_method` as a notification, then
    /// immediately the cheap debugStats probe, and assert the probe answers
    /// within `budget`. `notif_params` needs to make the handler's own work
    /// large enough that `budget` comfortably separates "handled inline"
    /// (which delays both the notification's own drain and the probe by
    /// roughly the handler's duration) from "deferred via spawn_blocking"
    /// (the exchange completes in low tens of milliseconds, dominated by
    /// transferring `notif_params` through the duplex stream rather than by
    /// any CPU-bound work).
    pub async fn assert_notification_stays_responsive(
        &mut self,
        notif_method: &str,
        notif_params: Value,
        budget: Duration,
    ) {
        let notif_method_owned = notif_method.to_owned();
        // The whole exchange — writing the notification, writing the probe,
        // and waiting for the probe's response — is under one clock. Writing
        // a large notification through the (size-bounded) duplex stream only
        // completes as fast as the server drains it; if the server is stuck
        // running the handler's work inline, that drain stalls too, so the
        // write time itself is part of what this budget is guarding.
        let elapsed = self
            .time_notification_and_probe(&notif_method_owned, notif_params)
            .await;
        assert!(
            elapsed <= budget,
            "{notif_method_owned} appears to block the request loop: opening this \
             document plus a follow-up debugStats probe did not complete within \
             {budget:?} (took {elapsed:?})"
        );
    }

    /// Send `notif_method` as a notification, then immediately a cheap
    /// `$/php-lsp/debugStats` probe, and return how long the whole exchange
    /// took to answer the probe. A generous 10s outer timeout guards against
    /// a genuine hang (vs. a slow-but-finite handler); it's not part of the
    /// signal callers reason about.
    ///
    /// Prefer this over `assert_notification_stays_responsive` when the
    /// operation's own JSON payload is large enough that transferring and
    /// decoding it dominates wall time — an absolute budget then mostly
    /// measures payload size, not whether the handler blocked the request
    /// loop. Measure a same-size no-op baseline with this method and compare
    /// the *delta* instead (see `did_open_stays_responsive_on_large_file`).
    pub async fn time_notification_and_probe(
        &mut self,
        notif_method: &str,
        notif_params: Value,
    ) -> Duration {
        let notif_method_owned = notif_method.to_owned();
        let start = std::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(10), async {
            self.notify(&notif_method_owned, notif_params).await;
            let probe_id = self.send_request("$/php-lsp/debugStats", json!({})).await;

            let mut already_buffered = self.pending.len();
            let TestClient {
                pending,
                read,
                write,
                ..
            } = self;
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut already_buffered).await;
                if msg.get("id") == Some(&json!(probe_id)) {
                    return;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{notif_method_owned} appears to block the request loop: opening this \
                 document plus a follow-up debugStats probe did not complete within 10s"
            )
        });
        start.elapsed()
    }

    pub async fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write.write_all(&frame(&msg)).await.unwrap();
    }

    /// Like `request`, but also collects every notification matching
    /// `capture_method` seen while waiting for the response (instead of
    /// silently discarding it, as `request` does). Returns
    /// `(response, captured_notifications)` in arrival order. Use for
    /// protocol tests asserting on server-sent notifications tied to a
    /// specific in-flight request (e.g. `$/progress` partial results).
    pub async fn request_capturing_notifications(
        &mut self,
        method: &str,
        params: Value,
        capture_method: &str,
    ) -> (Value, Vec<Value>) {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.write.write_all(&frame(&msg)).await.unwrap();
        let method_owned = method.to_owned();
        let mut captured = Vec::new();
        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        let resp = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("method") == Some(&json!(capture_method)) {
                    captured.push(msg);
                    continue;
                }
                if msg.get("id") == Some(&json!(id)) {
                    return msg;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for response to {method_owned}"));
        (resp, captured)
    }

    /// Block until a notification with `method` arrives. 5 s timeout.
    ///
    /// Forward-only (ignores anything already buffered): a bare method name
    /// can recur (e.g. multiple `window/logMessage`s in one session), so a
    /// historical match here could silently return a stale message instead
    /// of the fresh one this call actually wants — unlike the one-shot
    /// `$/php-lsp/indexReady`, there's no way to tell "the right one" apart
    /// from "an earlier one" by method name alone.
    pub async fn read_notification(&mut self, method: &str) -> Value {
        let mut budget = 0;
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("method") == Some(&json!(method)) {
                    return msg;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {method} notification"))
    }

    /// Block until `textDocument/publishDiagnostics` arrives for `uri`.
    /// Since `did_open` publishes diagnostics synchronously after parse +
    /// semantic analysis finish, this is a deterministic replacement for
    /// `sleep(150ms)` debounce waits.
    ///
    pub async fn wait_for_diagnostics(&mut self, uri: &str) -> Value {
        self.wait_for_diagnostics_secs(uri, 10).await
    }

    /// Same as [`Self::wait_for_diagnostics`], with a caller-chosen timeout.
    /// Use for setups where the publish can be delayed by a heavy background
    /// task (e.g. a large decoy workspace competing for the blocking pool).
    pub async fn wait_for_diagnostics_secs(&mut self, uri: &str, secs: u64) -> Value {
        let uri_val = json!(uri);
        // Forward-only: a uri's diagnostics can republish (e.g. after
        // `indexReady` or a later edit), so honoring a buffered match could
        // return a stale publish instead of the one this call is for.
        let mut budget = 0;
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(secs), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("method") == Some(&json!("textDocument/publishDiagnostics"))
                    && msg["params"]["uri"] == uri_val
                {
                    return msg;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for publishDiagnostics for {uri}"))
    }

    /// Wait for `textDocument/publishDiagnostics` for each of `uris`, in any
    /// order. Discards messages for other URIs encountered along the way.
    /// Returns a map keyed by URI. Replies to any server→client requests
    /// with `null` so the server isn't blocked while we're draining.
    ///
    /// Use when a single LSP event triggers publishes for multiple files
    /// (e.g., cross-file republish after a dependency change) and the test
    /// needs to assert against each independently.
    pub async fn wait_for_diagnostics_multi(
        &mut self,
        uris: &[&str],
    ) -> std::collections::HashMap<String, Value> {
        let mut remaining: std::collections::HashSet<String> =
            uris.iter().map(|s| s.to_string()).collect();
        let mut collected: std::collections::HashMap<String, Value> =
            std::collections::HashMap::new();
        let expected = remaining.clone();
        // Forward-only — see `wait_for_diagnostics` on why a repeatable
        // per-uri signal can't safely be satisfied from history.
        let mut budget = 0;
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(10), async {
            while !remaining.is_empty() {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                let wanted_uri = (msg.get("method")
                    == Some(&json!("textDocument/publishDiagnostics")))
                .then(|| msg["params"]["uri"].as_str())
                .flatten()
                .filter(|uri| remaining.contains(*uri))
                .map(|uri| uri.to_string());
                match wanted_uri {
                    Some(uri) => {
                        remaining.remove(&uri);
                        collected.insert(uri, msg);
                    }
                    None => pending.push_back(msg),
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!("timed out; expected publishDiagnostics for {expected:?}, got {collected:?}")
        });
        collected
    }

    /// Drain incoming messages for `duration`, returning every
    /// `publishDiagnostics` URI seen. Used to assert the *absence* of a
    /// publish (e.g., closed file must not receive cross-file republishes).
    pub async fn drain_publish_diagnostics_uris(&mut self, duration: Duration) -> Vec<String> {
        let mut uris = Vec::new();
        // Forward-only — this asserts on what arrives *during this call's
        // window*; a stale buffered publish from before it started isn't
        // part of that window and would just be noise.
        let mut budget = 0;
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        let _ = tokio::time::timeout(duration, async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("method") == Some(&json!("textDocument/publishDiagnostics")) {
                    if let Some(uri) = msg["params"]["uri"].as_str() {
                        uris.push(uri.to_string());
                    }
                } else {
                    pending.push_back(msg);
                }
            }
        })
        .await;
        uris
    }

    /// Read messages until a server→client request with the given `method`
    /// arrives. Returns `(id, params)`. Panics after 10 seconds.
    ///
    /// Deliberately does *not* touch `pending` and does *not* ACK any
    /// other message it passes over (matching this call's original,
    /// pre-buffering behavior exactly): some callers rely on an unrelated
    /// request (e.g. `client/registerCapability`, sent but never replied to
    /// until something acks it) staying un-acked for the rest of the
    /// session — acking it here would let whatever's awaiting that reply
    /// proceed and emit messages the test never anticipated, upsetting the
    /// ordering the strict `read_notification` callers depend on.
    pub async fn expect_server_request(&mut self, method: &str) -> (Value, Value) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = read_msg(&mut self.read).await;
                if msg.get("method") == Some(&json!(method)) && msg.get("id").is_some() {
                    let id = msg["id"].clone();
                    let params = msg.get("params").cloned().unwrap_or(json!(null));
                    return (id, params);
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for server request {method}"))
    }

    /// Send a successful response to a server→client request.
    pub async fn reply_to_server_request(&mut self, id: Value, result: Value) {
        let response = json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        });
        self.write.write_all(&frame(&response)).await.unwrap();
    }

    /// Drain messages until `$/php-lsp/indexReady` arrives, returning every
    /// notification seen along the way (server→client requests are acknowledged
    /// with `null` as usual). Useful for asserting protocol behavior during the
    /// workspace scan without racing against the ready signal.
    pub async fn collect_until_index_ready(&mut self) -> Vec<Value> {
        let mut notifications: Vec<Value> = Vec::new();
        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("method") == Some(&json!("$/php-lsp/indexReady")) {
                    return;
                }
                notifications.push(msg.clone());
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for $/php-lsp/indexReady"));
        notifications
    }

    /// Wait for `$/php-lsp/indexReady` with a custom timeout.
    /// Useful for large real-world codebases where 10 s is not enough.
    pub async fn wait_for_index_ready_secs(&mut self, secs: u64) {
        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(secs), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("method") == Some(&json!("$/php-lsp/indexReady")) {
                    return;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for $/php-lsp/indexReady after {secs}s"))
    }

    /// Wait for `$/php-lsp/indexReady` (10 s timeout). Auto-replies to any
    /// server-to-client requests sent during the workspace scan.
    pub async fn wait_for_index_ready(&mut self) {
        let mut budget = self.pending.len();
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("method") == Some(&json!("$/php-lsp/indexReady")) {
                    return;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for $/php-lsp/indexReady"))
    }

    /// Wait for a `window/logMessage` whose text starts with `prefix`.
    /// Used by flows (e.g. `workspace/didChangeConfiguration`) whose
    /// completion is only observable as a specific log line.
    ///
    /// Unlike `wait_for_index_ready` (a fires-once signal, safe to satisfy
    /// from history), this log line recurs — once at startup for
    /// auto-detection and again after every `didChangeConfiguration` — so
    /// matching against a buffered *past* occurrence would silently return
    /// a stale message instead of the one this specific call caused. Budget
    /// starts at 0 to force every message to come from a fresh (post-call)
    /// read; anything already buffered is left untouched for its rightful
    /// waiter.
    pub async fn wait_for_log_message_starting_with(&mut self, prefix: &str) -> Value {
        let mut budget = 0;
        let TestClient {
            pending,
            read,
            write,
            ..
        } = self;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let msg = recv_or_buffered(pending, read, write, &mut budget).await;
                if msg.get("method") == Some(&json!("window/logMessage"))
                    && msg["params"]["message"]
                        .as_str()
                        .unwrap_or("")
                        .starts_with(prefix)
                {
                    return msg;
                }
                pending.push_back(msg);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for log message starting with {prefix:?}"))
    }
}

pub(crate) fn spawn_server() -> TestClient {
    let (client_stream, server_stream) = tokio::io::duplex(1 << 20);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (service, socket) = LspService::build(Backend::new)
        .custom_method("$/php-lsp/debugStats", Backend::debug_stats)
        .finish();
    tokio::spawn(Server::new(server_read, server_write, socket).serve(service));
    TestClient {
        write: client_write,
        read: client_read,
        next_id: 1,
        pending: VecDeque::new(),
    }
}
