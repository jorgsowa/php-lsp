//! Windows clients disagree on how they spell a drive in a `file` URI
//! (VS Code sends `file:///c%3A/...`, others `file:///C:/...`), while every
//! URI the server builds from a path comes from `Uri::from_file_path`
//! (`file:///C%3A/...`). Incoming URIs are rewritten to that form so one file
//! never ends up under two keys.

use serde_json::Value;
use tower_lsp_server::jsonrpc::Request;

/// `request` with every drive-letter `file` URI in its params rewritten to
/// the `file:///C%3A/` form.
pub(super) fn normalize_request(request: Request) -> Request {
    if request.params().is_none_or(|p| !contains_drive_uri(p)) {
        return request;
    }
    let (method, id, params) = request.into_parts();
    let mut builder = Request::build(method);
    if let Some(mut params) = params {
        normalize_value(&mut params);
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

fn normalize_value(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(normalize_value),
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match v {
                    Value::String(s) if is_uri_key(key) => {
                        if let Some(normalized) = normalized_drive_uri(s) {
                            *s = normalized;
                        }
                    }
                    _ => normalize_value(v),
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
        let params = normalize_request(request).into_parts().2.unwrap();
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
        let params = normalize_request(request).into_parts().2.unwrap();
        assert_eq!(
            params,
            serde_json::json!({
                "files": [{"oldUri": "file:///D%3A/a.php", "newUri": "file:///D%3A/b.php"}]
            })
        );
    }
}
