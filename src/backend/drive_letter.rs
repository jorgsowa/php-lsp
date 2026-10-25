//! Windows clients disagree on how they spell a drive in a `file` URI
//! (VS Code sends `file:///c%3A/...`, others `file:///C:/...`), while every
//! URI the server builds from a path comes from `Uri::from_file_path`
//! (`file:///C%3A/...`). Incoming URIs are rewritten to that form so one file
//! never ends up under two keys, and outgoing ones go back to the client's
//! spelling, which is the key its own document model uses.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use futures::{Stream, StreamExt};
use serde_json::{Map, Value};
use tower_lsp_server::jsonrpc::{Request, Response};
use tower_lsp_server::{ClientSocket, Loopback};

/// The client's spelling of each drive-letter URI it sent, by canonical form.
#[derive(Clone, Default)]
pub(super) struct ClientSpellings(Arc<Mutex<HashMap<String, String>>>);

impl ClientSpellings {
    /// `request` with every drive-letter `file` URI in its params rewritten to
    /// the `file:///C%3A/` form.
    pub(super) fn normalize_request(&self, request: Request) -> Request {
        if request.params().is_none_or(|p| !contains_drive_uri(p)) {
            return request;
        }
        let mut seen = Vec::new();
        let request = map_params(request, |params| normalize_value(params, &mut seen));
        let mut spellings = self.0.lock().unwrap_or_else(|e| e.into_inner());
        for (canonical, original) in seen {
            spellings.entry(canonical).or_insert(original);
        }
        request
    }

    /// `request` to the client with the URIs it spelled differently restored.
    pub(super) fn restore_request(&self, request: Request) -> Request {
        let spellings = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if spellings.is_empty() || request.params().is_none() {
            return request;
        }
        map_params(request, |params| restore_value(params, &spellings))
    }

    /// Like [`Self::restore_request`], for a response to a client request.
    pub(super) fn restore_response(&self, response: Response) -> Response {
        let spellings = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if spellings.is_empty() {
            return response;
        }
        let (id, body) = response.into_parts();
        let body = body.map(|mut result| {
            restore_value(&mut result, &spellings);
            result
        });
        Response::from_parts(id, body)
    }
}

/// The server-to-client half of the transport, sending URIs in the client's spelling.
pub struct ClientLoopback {
    pub(super) socket: ClientSocket,
    pub(super) spellings: ClientSpellings,
}

impl Loopback for ClientLoopback {
    type RequestStream = Pin<Box<dyn Stream<Item = Request> + Send>>;
    type ResponseSink = <ClientSocket as Loopback>::ResponseSink;

    fn split(self) -> (Self::RequestStream, Self::ResponseSink) {
        let (requests, responses) = Loopback::split(self.socket);
        let spellings = self.spellings;
        (
            Box::pin(requests.map(move |request| spellings.restore_request(request))),
            responses,
        )
    }
}

fn map_params(request: Request, f: impl FnOnce(&mut Value)) -> Request {
    let (method, id, params) = request.into_parts();
    let mut builder = Request::build(method);
    if let Some(mut params) = params {
        f(&mut params);
        builder = builder.params(params);
    }
    if let Some(id) = id {
        builder = builder.id(id);
    }
    builder.finish()
}

fn is_uri_key(key: &str) -> bool {
    key.len() >= 3 && key[key.len() - 3..].eq_ignore_ascii_case("uri")
}

/// Only values under a `uri`-suffixed key are touched, so document text that
/// happens to hold a URI is left alone.
fn contains_drive_uri(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(contains_drive_uri),
        Value::Object(map) => map.iter().any(|(key, v)| match v {
            Value::String(s) => is_uri_key(key) && normalized_drive_uri(s).is_some(),
            _ => contains_drive_uri(v),
        }),
        _ => false,
    }
}

/// Rewrites drive-letter URIs in place, noting each `(canonical, original)` in `seen`.
fn normalize_value(value: &mut Value, seen: &mut Vec<(String, String)>) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|v| normalize_value(v, seen)),
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match v {
                    Value::String(s) if is_uri_key(key) => {
                        if let Some(normalized) = normalized_drive_uri(s) {
                            let original = std::mem::replace(s, normalized.clone());
                            seen.push((normalized, original));
                        }
                    }
                    _ => normalize_value(v, seen),
                }
            }
        }
        _ => {}
    }
}

/// Puts back the client's spelling under `uri`-suffixed keys and in the
/// URI-keyed `changes` map of a `WorkspaceEdit`.
fn restore_value(value: &mut Value, spellings: &HashMap<String, String>) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|v| restore_value(v, spellings)),
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match v {
                    Value::String(s) if is_uri_key(key) => {
                        if let Some(original) = spellings.get(s.as_str()) {
                            s.clone_from(original);
                        }
                    }
                    Value::Object(changes) if key == "changes" => {
                        *changes = std::mem::take(changes)
                            .into_iter()
                            .map(|(uri, mut edits)| {
                                restore_value(&mut edits, spellings);
                                (spellings.get(&uri).cloned().unwrap_or(uri), edits)
                            })
                            .collect::<Map<_, _>>();
                    }
                    _ => restore_value(v, spellings),
                }
            }
        }
        _ => {}
    }
}

/// `Some` when `s` is a drive-letter `file` URI not already in canonical form.
fn normalized_drive_uri(s: &str) -> Option<String> {
    let rest = s.strip_prefix("file:///")?;
    let drive = *rest.as_bytes().first()?;
    if !drive.is_ascii_alphabetic() {
        return None;
    }
    let after = &rest[1..];
    let tail = after
        .strip_prefix(':')
        .or_else(|| after.strip_prefix("%3A"))
        .or_else(|| after.strip_prefix("%3a"))?;
    if !(tail.is_empty() || tail.starts_with('/')) {
        return None;
    }
    let normalized = format!("file:///{}%3A{tail}", drive.to_ascii_uppercase() as char);
    (normalized != s).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_spellings_normalize_to_upper_case_encoded_colon() {
        for input in [
            "file:///c%3A/proj/a.php",
            "file:///c:/proj/a.php",
            "file:///C:/proj/a.php",
            "file:///c%3a/proj/a.php",
        ] {
            assert_eq!(
                normalized_drive_uri(input).as_deref(),
                Some("file:///C%3A/proj/a.php"),
                "{input}"
            );
        }
        assert_eq!(normalized_drive_uri("file:///C%3A/proj/a.php"), None);
        assert_eq!(normalized_drive_uri("file:///home/a.php"), None);
        assert_eq!(normalized_drive_uri("file:///ab:/x"), None);
        assert_eq!(normalized_drive_uri("<?php file:///c:/x"), None);
    }

    #[test]
    fn document_text_holding_a_uri_is_untouched() {
        let params = serde_json::json!({
            "textDocument": {"uri": "file:///c:/a.php", "version": 2},
            "contentChanges": [{"text": "file:///c:/pasted"}]
        });
        let request = Request::build("textDocument/didChange")
            .params(params)
            .finish();
        let params = ClientSpellings::default()
            .normalize_request(request)
            .into_parts()
            .2
            .unwrap();
        assert_eq!(params["textDocument"]["uri"], "file:///C%3A/a.php");
        assert_eq!(params["contentChanges"][0]["text"], "file:///c:/pasted");
    }

    #[test]
    fn nested_params_are_rewritten() {
        let request = Request::build("workspace/didRenameFiles")
            .params(serde_json::json!({
                "files": [{"oldUri": "file:///d:/a.php", "newUri": "file:///d%3A/b.php"}]
            }))
            .finish();
        let params = ClientSpellings::default()
            .normalize_request(request)
            .into_parts()
            .2
            .unwrap();
        assert_eq!(
            params,
            serde_json::json!({
                "files": [{"oldUri": "file:///D%3A/a.php", "newUri": "file:///D%3A/b.php"}]
            })
        );
    }

    #[test]
    fn outgoing_uris_use_the_client_spelling() {
        let spellings = ClientSpellings::default();
        spellings.normalize_request(
            Request::build("textDocument/didOpen")
                .params(serde_json::json!({"textDocument": {"uri": "file:///c%3A/a.php"}}))
                .finish(),
        );
        let request = Request::build("workspace/applyEdit")
            .params(serde_json::json!({
                "edit": {"changes": {"file:///C%3A/a.php": [], "file:///C%3A/b.php": []}},
                "uri": "file:///C%3A/a.php"
            }))
            .finish();
        let params = spellings.restore_request(request).into_parts().2.unwrap();
        assert_eq!(
            params,
            serde_json::json!({
                "edit": {"changes": {"file:///c%3A/a.php": [], "file:///C%3A/b.php": []}},
                "uri": "file:///c%3A/a.php"
            })
        );
    }
}
