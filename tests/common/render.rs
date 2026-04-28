#![allow(dead_code)]

use serde_json::Value;

// ---------- snapshot canonicalization ----------

/// Render a WorkspaceEdit (the `result` payload from `textDocument/rename`,
/// `workspace/willRenameFiles`, a resolved codeAction, …) into a stable
/// text form suitable for `expect_test` snapshots:
///
/// - URIs stripped of the `file://<root>/` prefix so tempdir paths don't
///   leak into snapshots
/// - Per-file edits sorted by `(line, character, end_line, end_character)`
///   so the output is deterministic regardless of server-side ordering
/// - Each edit rendered as `L:C-L:C → "newText"` with `\n`/`\r` visible
///
/// Pass `root_uri` as the `file://…` URI of the workspace root (i.e. what
/// `TestServer::uri("")` returns). Pass an empty string to keep full URIs.
pub fn canonicalize_workspace_edit(edit: &Value, root_uri: &str) -> String {
    let Some(changes) = edit["changes"].as_object() else {
        return format!("<no `changes` map in {edit}>");
    };

    // Trailing slash so "file:///tmp/x" + "a.php" → "a.php", not "/a.php".
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };

    let mut uris: Vec<&String> = changes.keys().collect();
    uris.sort();

    let mut out = String::new();
    for uri in uris {
        let short = uri.strip_prefix(&prefix).unwrap_or(uri);
        out.push_str(&format!("// {short}\n"));

        let mut edits: Vec<&Value> = changes[uri]
            .as_array()
            .map(|a| a.iter().collect())
            .unwrap_or_default();
        edits.sort_by_key(|e| {
            (
                e["range"]["start"]["line"].as_u64().unwrap_or(0),
                e["range"]["start"]["character"].as_u64().unwrap_or(0),
                e["range"]["end"]["line"].as_u64().unwrap_or(0),
                e["range"]["end"]["character"].as_u64().unwrap_or(0),
            )
        });
        for e in edits {
            let s = &e["range"]["start"];
            let en = &e["range"]["end"];
            let text = e["newText"].as_str().unwrap_or("");
            out.push_str(&format!(
                "{}:{}-{}:{} → {:?}\n",
                s["line"].as_u64().unwrap_or(0),
                s["character"].as_u64().unwrap_or(0),
                en["line"].as_u64().unwrap_or(0),
                en["character"].as_u64().unwrap_or(0),
                text,
            ));
        }
        out.push('\n');
    }
    out.trim_end_matches('\n').to_owned()
}

// ---------- symbol / kind name tables ----------

fn symbol_kind_name(k: u64) -> &'static str {
    match k {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "?",
    }
}

fn completion_kind_name(k: u64) -> &'static str {
    match k {
        1 => "Text",
        2 => "Method",
        3 => "Function",
        4 => "Constructor",
        5 => "Field",
        6 => "Variable",
        7 => "Class",
        8 => "Interface",
        9 => "Module",
        10 => "Property",
        11 => "Unit",
        12 => "Value",
        13 => "Enum",
        14 => "Keyword",
        15 => "Snippet",
        16 => "Color",
        17 => "File",
        18 => "Reference",
        19 => "Folder",
        20 => "EnumMember",
        21 => "Constant",
        22 => "Struct",
        23 => "Event",
        24 => "Operator",
        25 => "TypeParameter",
        _ => "?",
    }
}

// ---------- render helpers ----------

pub fn render_document_symbols(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no symbols>".to_owned();
    }
    let mut out = String::new();
    fn walk(out: &mut String, sym: &Value, depth: usize) {
        let name = sym["name"].as_str().unwrap_or("?");
        let kind = symbol_kind_name(sym["kind"].as_u64().unwrap_or(0));
        let r = &sym["selectionRange"];
        let line = r["start"]["line"].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "{:indent$}{kind} {name} @L{line}\n",
            "",
            indent = depth * 2,
        ));
        if let Some(children) = sym["children"].as_array() {
            for child in children {
                walk(out, child, depth + 1);
            }
        }
    }
    for sym in &arr {
        walk(&mut out, sym, 0);
    }
    out.trim_end().to_owned()
}

pub fn render_workspace_symbols(resp: &Value, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no symbols>".to_owned();
    }
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let mut rows: Vec<String> = arr
        .iter()
        .map(|s| {
            let name = s["name"].as_str().unwrap_or("?");
            let kind = symbol_kind_name(s["kind"].as_u64().unwrap_or(0));
            let uri = s["location"]["uri"].as_str().unwrap_or("?");
            let short = uri.strip_prefix(&prefix).unwrap_or(uri);
            let line = s["location"]["range"]["start"]["line"]
                .as_u64()
                .unwrap_or(0);
            format!("{kind:<11} {name} @ {short}:{line}")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

/// Render a `Location[]` / `Location` / `LocationLink[]` response as sorted
/// `path:line:col-line:col` lines. Unknown / null results produce
/// `<none>` for readability.
pub(crate) fn render_locations(resp: &Value, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<none>".to_owned();
    }
    let locs: Vec<Value> = if result.is_array() {
        result.as_array().cloned().unwrap_or_default()
    } else {
        vec![result.clone()]
    };
    if locs.is_empty() {
        return "<none>".to_owned();
    }
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let mut rows: Vec<String> = locs
        .iter()
        .map(|l| {
            // LocationLink uses `targetUri`/`targetRange`; Location uses `uri`/`range`.
            let uri = l["uri"]
                .as_str()
                .or_else(|| l["targetUri"].as_str())
                .unwrap_or("?");
            let short = uri.strip_prefix(&prefix).unwrap_or(uri);
            let r = if l["range"].is_object() {
                &l["range"]
            } else {
                &l["targetRange"]
            };
            format!(
                "{short}:{}:{}-{}:{}",
                r["start"]["line"].as_u64().unwrap_or(0),
                r["start"]["character"].as_u64().unwrap_or(0),
                r["end"]["line"].as_u64().unwrap_or(0),
                r["end"]["character"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

pub fn render_hover(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<no hover>".to_owned();
    }
    let value = result["contents"]["value"].as_str().unwrap_or_default();
    value
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn render_completion(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let items: Vec<Value> = match &resp["result"] {
        v if v.is_array() => v.as_array().cloned().unwrap_or_default(),
        v if v["items"].is_array() => v["items"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    };
    if items.is_empty() {
        return "<no completions>".to_owned();
    }
    let mut rows: Vec<(String, String)> = items
        .iter()
        .map(|i| {
            let label = i["label"].as_str().unwrap_or("?");
            let kind = completion_kind_name(i["kind"].as_u64().unwrap_or(0));
            let sort = i["sortText"].as_str().unwrap_or(label).to_owned();
            (sort, format!("{kind:<11} {label}"))
        })
        .collect();
    rows.sort();
    rows.into_iter()
        .map(|(_, r)| r)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn render_signature_help(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<no signature>".to_owned();
    }
    let sigs = result["signatures"].as_array().cloned().unwrap_or_default();
    if sigs.is_empty() {
        return "<no signature>".to_owned();
    }
    let active_sig = result["activeSignature"].as_u64().unwrap_or(0) as usize;
    let active_param = result["activeParameter"].as_u64();
    let mut out = String::new();
    for (i, sig) in sigs.iter().enumerate() {
        let label = sig["label"].as_str().unwrap_or("");
        let marker = if i == active_sig { "▶ " } else { "  " };
        out.push_str(&format!("{marker}{label}"));
        if i == active_sig {
            if let Some(p) = active_param {
                out.push_str(&format!("  @param{p}"));
            }
        }
        out.push('\n');
    }
    out.trim_end().to_owned()
}

pub fn render_inlay_hints(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no hints>".to_owned();
    }
    let mut rows: Vec<(u64, u64, String)> = arr
        .iter()
        .map(|h| {
            let line = h["position"]["line"].as_u64().unwrap_or(0);
            let col = h["position"]["character"].as_u64().unwrap_or(0);
            // label may be a string or a LabelPart[]
            let label = match &h["label"] {
                Value::String(s) => s.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|p| p["value"].as_str())
                    .collect::<Vec<_>>()
                    .join(""),
                _ => String::new(),
            };
            (line, col, label)
        })
        .collect();
    rows.sort();
    rows.into_iter()
        .map(|(l, c, label)| format!("{l}:{c} {label}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn render_code_actions(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no actions>".to_owned();
    }
    let mut rows: Vec<String> = arr
        .iter()
        .map(|a| {
            let title = a["title"].as_str().unwrap_or("?");
            let kind = a["kind"].as_str().unwrap_or("");
            if kind.is_empty() {
                title.to_owned()
            } else {
                format!("{kind:<16} {title}")
            }
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

pub(crate) fn render_folding_ranges(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no folds>".to_owned();
    }
    let mut rows: Vec<String> = arr
        .iter()
        .map(|r| {
            let sl = r["startLine"].as_u64().unwrap_or(0);
            let el = r["endLine"].as_u64().unwrap_or(0);
            let kind = r["kind"].as_str().unwrap_or("region");
            format!("{sl}..{el} {kind}")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

pub(crate) fn render_code_lens(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no lens>".to_owned();
    }
    let mut rows: Vec<String> = arr
        .iter()
        .map(|l| {
            let sl = l["range"]["start"]["line"].as_u64().unwrap_or(0);
            let title = l["command"]["title"].as_str().unwrap_or("<unresolved>");
            let cmd = l["command"]["command"].as_str().unwrap_or("");
            format!("L{sl}: {title} [{cmd}]")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

pub(crate) fn render_type_hierarchy(resp: &Value, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<empty>".to_owned();
    }
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let mut rows: Vec<String> = arr
        .iter()
        .map(|i| {
            let name = i["name"].as_str().unwrap_or("?");
            let kind = symbol_kind_name(i["kind"].as_u64().unwrap_or(0));
            let uri = i["uri"].as_str().unwrap_or("?");
            let short = uri.strip_prefix(&prefix).unwrap_or(uri);
            let line = i["selectionRange"]["start"]["line"].as_u64().unwrap_or(0);
            format!("{name} ({kind}) @ {short}:{line}")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

pub(crate) fn render_prepare_rename(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let r = &resp["result"];
    if r.is_null() {
        return "<not renameable>".to_owned();
    }
    // Either a Range ({start,end}) or { range, placeholder }.
    let range = if r["range"].is_object() {
        &r["range"]
    } else {
        r
    };
    let placeholder = r["placeholder"].as_str();
    let out = format!(
        "{}:{}-{}:{}",
        range["start"]["line"].as_u64().unwrap_or(0),
        range["start"]["character"].as_u64().unwrap_or(0),
        range["end"]["line"].as_u64().unwrap_or(0),
        range["end"]["character"].as_u64().unwrap_or(0),
    );
    match placeholder {
        Some(p) => format!("{out} {p}"),
        None => out,
    }
}

pub(crate) fn render_prepare_call_hierarchy(resp: &Value, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<empty>".to_owned();
    }
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let mut rows: Vec<String> = arr
        .iter()
        .map(|i| {
            let name = i["name"].as_str().unwrap_or("?");
            let kind = symbol_kind_name(i["kind"].as_u64().unwrap_or(0));
            let uri = i["uri"].as_str().unwrap_or("?");
            let short = uri.strip_prefix(&prefix).unwrap_or(uri);
            let line = i["selectionRange"]["start"]["line"].as_u64().unwrap_or(0);
            match i["detail"].as_str() {
                Some(detail) => format!("{name} ({kind}) [{detail}] @ {short}:{line}"),
                None => format!("{name} ({kind}) @ {short}:{line}"),
            }
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

pub(crate) fn render_call_hierarchy(resp: &Value, side: &str, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no calls>".to_owned();
    }
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let mut rows: Vec<String> = arr
        .iter()
        .map(|c| {
            let node = &c[side];
            let name = node["name"].as_str().unwrap_or("?");
            let uri = node["uri"].as_str().unwrap_or("?");
            let short = uri.strip_prefix(&prefix).unwrap_or(uri);
            let line = node["selectionRange"]["start"]["line"]
                .as_u64()
                .or_else(|| node["range"]["start"]["line"].as_u64())
                .unwrap_or(0);
            format!("{name} @ {short}:{line}")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

// ---------- annotation-based assertion helpers ----------

/// Collect `// ^^^ <tag>` annotations across every fixture file, filtered by
/// the set of accepted tags. Each returned tuple is `(path, range, tag)`.
pub(crate) fn collect_navigation_annotations(
    fx: &super::fixture::Fixture,
    accept: &[&str],
) -> Vec<(String, (u32, u32, u32, u32), String)> {
    let mut out = Vec::new();
    for file in &fx.files {
        for anno in &file.annotations {
            if accept.iter().any(|a| *a == anno.message) {
                out.push((
                    file.path.clone(),
                    (anno.line, anno.start_char, anno.line, anno.end_char),
                    anno.message.clone(),
                ));
            }
        }
    }
    out
}

#[track_caller]
pub(crate) fn assert_locations_match(
    resp: &Value,
    expected: &[(String, (u32, u32, u32, u32), String)],
    root_uri: &str,
    label: &str,
) {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        panic!("{label} request errored: {err}");
    }
    let result = &resp["result"];
    let locs: Vec<Value> = if result.is_array() {
        result.as_array().cloned().unwrap_or_default()
    } else if result.is_null() {
        vec![]
    } else {
        vec![result.clone()]
    };
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let actual: Vec<(String, (u32, u32, u32, u32))> = locs
        .iter()
        .map(|l| {
            let uri = l["uri"]
                .as_str()
                .or_else(|| l["targetUri"].as_str())
                .unwrap_or("?");
            let short = uri.strip_prefix(&prefix).unwrap_or(uri).to_owned();
            let r = if l["range"].is_object() {
                &l["range"]
            } else {
                &l["targetRange"]
            };
            (
                short,
                (
                    r["start"]["line"].as_u64().unwrap_or(0) as u32,
                    r["start"]["character"].as_u64().unwrap_or(0) as u32,
                    r["end"]["line"].as_u64().unwrap_or(0) as u32,
                    r["end"]["character"].as_u64().unwrap_or(0) as u32,
                ),
            )
        })
        .collect();
    let mut matched = vec![false; actual.len()];
    let mut missing = Vec::new();
    for (ep, er, tag) in expected {
        // Accept any server-returned range that overlaps the annotation
        // caret span on the same line & file — servers vary on whether
        // they return identifier spans or whole-statement spans.
        let hit = actual
            .iter()
            .enumerate()
            .position(|(i, (ap, ar))| !matched[i] && ap == ep && ranges_overlap_same_line(er, ar));
        match hit {
            Some(i) => matched[i] = true,
            None => missing.push((ep.clone(), *er, tag.clone())),
        }
    }
    let extras: Vec<_> = actual
        .iter()
        .enumerate()
        .filter(|(i, _)| !matched[*i])
        .map(|(_, v)| v.clone())
        .collect();
    if !missing.is_empty() || !extras.is_empty() {
        panic!(
            "{label} mismatch\nexpected (missing): {missing:#?}\nactual (unmatched): {extras:#?}\nfull: {resp}"
        );
    }
}

/// Is the expected caret line covered by the actual range? We intentionally
/// do not compare character columns: server implementations vary widely on
/// whether they return identifier spans, statement spans, or degenerate
/// zero-width ranges. Line-granular matching still catches "wrong file" and
/// "wrong line" regressions, which is what callers care about.
fn ranges_overlap_same_line(
    expected: &(u32, u32, u32, u32),
    actual: &(u32, u32, u32, u32),
) -> bool {
    let (esl, _, _, _) = *expected;
    let (asl, _, ael, _) = *actual;
    esl >= asl && esl <= ael
}

#[track_caller]
pub(crate) fn assert_highlights_match(
    resp: &Value,
    expected: &[(String, (u32, u32, u32, u32), String)],
    cursor_path: &str,
    label: &str,
) {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        panic!("{label} request errored: {err}");
    }
    let locs = resp["result"].as_array().cloned().unwrap_or_default();
    let actual: Vec<(u32, u32, u32, u32)> = locs
        .iter()
        .map(|l| {
            let r = &l["range"];
            (
                r["start"]["line"].as_u64().unwrap_or(0) as u32,
                r["start"]["character"].as_u64().unwrap_or(0) as u32,
                r["end"]["line"].as_u64().unwrap_or(0) as u32,
                r["end"]["character"].as_u64().unwrap_or(0) as u32,
            )
        })
        .collect();
    let expected_ranges: Vec<(u32, u32, u32, u32)> = expected
        .iter()
        .filter(|(p, _, _)| p == cursor_path)
        .map(|(_, r, _)| *r)
        .collect();
    let mut matched = vec![false; actual.len()];
    let mut missing = Vec::new();
    for er in &expected_ranges {
        let hit = actual
            .iter()
            .enumerate()
            .position(|(i, ar)| !matched[i] && ranges_overlap_same_line(er, ar));
        match hit {
            Some(i) => matched[i] = true,
            None => missing.push(*er),
        }
    }
    let extras: Vec<_> = actual
        .iter()
        .enumerate()
        .filter(|(i, _)| !matched[*i])
        .map(|(_, v)| *v)
        .collect();
    if !missing.is_empty() || !extras.is_empty() {
        panic!(
            "{label} mismatch\nexpected (missing): {missing:#?}\nactual (unmatched): {extras:#?}\nfull: {resp}"
        );
    }
}
