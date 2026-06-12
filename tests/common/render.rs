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

/// Render a TextEdit array (the `result` payload from `textDocument/formatting`,
/// `textDocument/rangeFormatting`, `textDocument/willSaveWaitUntil`, etc.) into
/// a stable snapshot form suitable for `expect_test`:
///
/// - Each edit rendered as `L:C-L:C → "newText"` with `\n` visible as `\n` and `\r` as `\r`
/// - Edits sorted by position for deterministic output
/// - Handles null result (no formatter available) separately
pub fn render_text_edits(resp: &Value) -> String {
    let result = &resp["result"];
    if result.is_null() {
        return "(no formatter available)".to_owned();
    }

    let Some(edits) = result.as_array() else {
        return format!("<invalid TextEdit array: {result:?}>");
    };

    if edits.is_empty() {
        return "(no changes needed)".to_owned();
    }

    let mut sorted: Vec<&Value> = edits.iter().collect();
    sorted.sort_by_key(|e| {
        (
            e["range"]["start"]["line"].as_u64().unwrap_or(0),
            e["range"]["start"]["character"].as_u64().unwrap_or(0),
            e["range"]["end"]["line"].as_u64().unwrap_or(0),
            e["range"]["end"]["character"].as_u64().unwrap_or(0),
        )
    });

    let mut out = String::new();
    for e in sorted {
        let s = &e["range"]["start"];
        let en = &e["range"]["end"];
        let text = e["newText"].as_str().unwrap_or("");
        // Escape newlines for visibility in snapshots
        let escaped = text.replace('\n', "\\n").replace('\r', "\\r");
        out.push_str(&format!(
            "{}:{}-{}:{} → {}\n",
            s["line"].as_u64().unwrap_or(0),
            s["character"].as_u64().unwrap_or(0),
            en["line"].as_u64().unwrap_or(0),
            en["character"].as_u64().unwrap_or(0),
            format!("{:?}", escaped),
        ));
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
pub fn render_locations(resp: &Value, root_uri: &str) -> String {
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
    let contents = &result["contents"];
    // Handle all three LSP Hover.contents variants:
    // 1. MarkupContent { kind, value }
    // 2. MarkedString (plain string)
    // 3. MarkedString[] (array of strings or { language, value })
    let value = if let Some(s) = contents.as_str() {
        // MarkedString as plain string (deprecated but valid)
        s.to_owned()
    } else if let Some(arr) = contents.as_array() {
        // MarkedString[] (deprecated but valid)
        arr.iter()
            .map(|item| {
                if let Some(s) = item.as_str() {
                    s.to_owned()
                } else {
                    item["value"].as_str().unwrap_or("").to_owned()
                }
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n---\n")
    } else {
        // MarkupContent { kind, value } — current and preferred form
        contents["value"].as_str().unwrap_or("").to_owned()
    };
    value
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_completion(resp: &Value) -> String {
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

/// Full-detail completion snapshot that includes kind, label, sortText, and detail.
/// Items are sorted by sortText for deterministic snapshots. Use this for
/// comprehensive assertions that verify all completion properties and ordering.
pub fn render_completion_ordered(resp: &Value) -> String {
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
            let detail = i["detail"].as_str().unwrap_or("");
            let detail_suffix = if detail.is_empty() {
                String::new()
            } else {
                format!(" | {detail}")
            };
            (sort.clone(), format!("{kind:<11} {label}{detail_suffix}"))
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
            // Add suffix indicating whether the action has an edit or command.
            let edit_marker = if a["edit"].is_object() {
                " [edit]"
            } else if a["command"].is_object() {
                " [cmd]"
            } else {
                ""
            };
            if kind.is_empty() {
                format!("{title}{edit_marker}")
            } else {
                format!("{kind:<16} {title}{edit_marker}")
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
            // Include character boundaries (LSP 3.17+) if present and non-zero.
            let range_str = match (r["startCharacter"].as_u64(), r["endCharacter"].as_u64()) {
                (Some(sc), Some(ec)) if sc > 0 || ec > 0 => {
                    format!("{sl}:{sc}..{el}:{ec}")
                }
                _ => format!("{sl}..{el}"),
            };
            format!("{range_str} {kind}")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

/// Verify the LSP-spec invariant: for every chain in a
/// `textDocument/selectionRange` response, every parent range fully
/// contains its child. Panics with a descriptive message otherwise.
pub fn assert_selection_range_invariant(resp: &Value) {
    let Some(arr) = resp["result"].as_array() else {
        return;
    };
    fn r_to_tuple(r: &Value) -> (u64, u64, u64, u64) {
        let s = &r["start"];
        let e = &r["end"];
        (
            s["line"].as_u64().unwrap_or(0),
            s["character"].as_u64().unwrap_or(0),
            e["line"].as_u64().unwrap_or(0),
            e["character"].as_u64().unwrap_or(0),
        )
    }
    fn contains(parent: (u64, u64, u64, u64), child: (u64, u64, u64, u64)) -> bool {
        (parent.0, parent.1) <= (child.0, child.1) && (child.2, child.3) <= (parent.2, parent.3)
    }
    for (i, chain) in arr.iter().enumerate() {
        let mut node = chain;
        loop {
            let parent = &node["parent"];
            if !parent.is_object() {
                break;
            }
            let cr = r_to_tuple(&node["range"]);
            let pr = r_to_tuple(&parent["range"]);
            assert!(
                contains(pr, cr),
                "chain[{i}]: parent {pr:?} does not contain child {cr:?}"
            );
            assert!(
                pr != cr,
                "chain[{i}]: parent and child have identical range {pr:?} — selection ranges must be strictly nested"
            );
            node = parent;
        }
    }
}

/// Verify the LSP-spec invariant for `textDocument/documentSymbol`: every
/// symbol's `selectionRange` must be fully contained in its `range`.
/// Panics with a descriptive message on violation.
pub fn assert_document_symbol_containment(resp: &Value) {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        panic!("documentSymbol request failed: {err}");
    }
    let Some(arr) = resp["result"].as_array() else {
        panic!(
            "documentSymbol: result must be an array, got: {}",
            resp["result"]
        );
    };
    fn pos(p: &Value) -> (u64, u64) {
        (
            p["line"].as_u64().unwrap_or(0),
            p["character"].as_u64().unwrap_or(0),
        )
    }
    fn check(sym: &Value) {
        let name = sym["name"].as_str().unwrap_or("?");
        let r_start = pos(&sym["range"]["start"]);
        let r_end = pos(&sym["range"]["end"]);
        let s_start = pos(&sym["selectionRange"]["start"]);
        let s_end = pos(&sym["selectionRange"]["end"]);
        assert!(
            r_start <= s_start && s_end <= r_end,
            "symbol {name:?}: selectionRange {s_start:?}-{s_end:?} not contained in fullRange {r_start:?}-{r_end:?}"
        );
        if let Some(children) = sym["children"].as_array() {
            for child in children {
                check(child);
            }
        }
    }
    for sym in arr {
        check(sym);
    }
}

/// Render a `textDocument/selectionRange` response as one chain per request
/// position. Each chain prints innermost → outermost as `L:C-L:C` lines, one
/// per parent step. Multiple chains are separated by `---`.
pub(crate) fn render_selection_range(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let arr = resp["result"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no selection ranges>".to_owned();
    }
    let mut chains: Vec<String> = Vec::with_capacity(arr.len());
    for chain in arr.iter() {
        let mut lines: Vec<String> = Vec::new();
        let mut node = chain;
        loop {
            let r = &node["range"];
            let sl = r["start"]["line"].as_u64().unwrap_or(0);
            let sc = r["start"]["character"].as_u64().unwrap_or(0);
            let el = r["end"]["line"].as_u64().unwrap_or(0);
            let ec = r["end"]["character"].as_u64().unwrap_or(0);
            lines.push(format!("{sl}:{sc}-{el}:{ec}"));
            let parent = &node["parent"];
            if !parent.is_object() {
                break;
            }
            node = parent;
        }
        chains.push(lines.join("\n"));
    }
    chains.join("\n---\n")
}

/// LSP-spec invariant: every range in a `LinkedEditingRanges` response
/// must cover the *same text*, since linked-mode typing replicates one
/// edit across all of them. This re-extracts each range's content from
/// `source` and asserts they all match.
pub fn assert_linked_editing_ranges_share_text(resp: &Value, source: &str) {
    let result = &resp["result"];
    if result.is_null() {
        return;
    }
    let Some(arr) = result["ranges"].as_array() else {
        return;
    };
    let lines: Vec<&str> = source.split('\n').collect();
    let extract = |r: &Value| -> Option<String> {
        let sl = r["start"]["line"].as_u64()? as usize;
        let sc = r["start"]["character"].as_u64()? as usize;
        let el = r["end"]["line"].as_u64()? as usize;
        let ec = r["end"]["character"].as_u64()? as usize;
        if sl != el {
            return Some(format!("<multiline {sl}:{sc}-{el}:{ec}>"));
        }
        let line = lines.get(sl)?;
        let chars: Vec<char> = line.chars().collect();
        // Column-to-char index walk using UTF-16 column semantics:
        let mut start_idx = 0usize;
        let mut col = 0usize;
        for (i, ch) in chars.iter().enumerate() {
            if col >= sc {
                start_idx = i;
                break;
            }
            col += ch.len_utf16() as usize;
            start_idx = i + 1;
        }
        let mut end_idx = start_idx;
        let mut col = sc;
        for (i, ch) in chars.iter().enumerate().skip(start_idx) {
            if col >= ec {
                end_idx = i;
                break;
            }
            col += ch.len_utf16() as usize;
            end_idx = i + 1;
        }
        Some(chars[start_idx..end_idx].iter().collect())
    };
    let texts: Vec<String> = arr.iter().filter_map(extract).collect();
    if texts.len() <= 1 {
        return;
    }
    let first = &texts[0];
    for (i, t) in texts.iter().enumerate().skip(1) {
        assert_eq!(
            t, first,
            "linked-editing range[{i}] text {t:?} differs from first {first:?}"
        );
    }
}

/// Render a `textDocument/linkedEditingRange` response: one range per line
/// as `L:C-L:C`, sorted by start position; the word pattern is appended on
/// a final `pattern: …` line. `<no linked editing>` for null/empty.
pub(crate) fn render_linked_editing_range(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<no linked editing>".to_owned();
    }
    let arr = result["ranges"].as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no linked editing>".to_owned();
    }
    let mut rows: Vec<(u64, u64, String)> = arr
        .iter()
        .map(|r| {
            let sl = r["start"]["line"].as_u64().unwrap_or(0);
            let sc = r["start"]["character"].as_u64().unwrap_or(0);
            let el = r["end"]["line"].as_u64().unwrap_or(0);
            let ec = r["end"]["character"].as_u64().unwrap_or(0);
            (sl, sc, format!("{sl}:{sc}-{el}:{ec}"))
        })
        .collect();
    rows.sort_by_key(|(sl, sc, _)| (*sl, *sc));
    let mut out: Vec<String> = rows.into_iter().map(|(_, _, s)| s).collect();
    if let Some(p) = result["wordPattern"].as_str() {
        out.push(format!("pattern: {p}"));
    }
    out.join("\n")
}

/// Render a `textDocument/moniker` response — one moniker per line as
/// `<scheme>:<identifier> kind=<kind> unique=<unique>`. The string variants
/// of `kind` and `unique` come straight from the JSON; missing optional
/// fields render as `<unset>`.
pub(crate) fn render_moniker(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<no moniker>".to_owned();
    }
    let arr = result.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no moniker>".to_owned();
    }
    let mut rows: Vec<String> = arr
        .iter()
        .map(|m| {
            let scheme = m["scheme"].as_str().unwrap_or("<unset>");
            let identifier = m["identifier"].as_str().unwrap_or("<unset>");
            let kind = m["kind"].as_str().unwrap_or("<unset>");
            let unique = m["unique"].as_str().unwrap_or("<unset>");
            format!("{scheme}:{identifier} kind={kind} unique={unique}")
        })
        .collect();
    rows.sort();
    rows.join("\n")
}

/// Render a `textDocument/inlineValue` response — one `VariableLookup` per
/// line as `L:C-L:C $name (case_sensitive)` sorted by start position so
/// the snapshot is order-independent. Other inline-value variants render
/// their tag plus the range.
pub(crate) fn render_inline_value(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<no inline values>".to_owned();
    }
    let arr = result.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "<no inline values>".to_owned();
    }
    let mut rows: Vec<(u64, u64, String)> = arr
        .iter()
        .map(|v| {
            let r = &v["range"];
            let sl = r["start"]["line"].as_u64().unwrap_or(0);
            let sc = r["start"]["character"].as_u64().unwrap_or(0);
            let el = r["end"]["line"].as_u64().unwrap_or(0);
            let ec = r["end"]["character"].as_u64().unwrap_or(0);
            // VariableLookup has variableName + caseSensitiveLookup.
            let line = if let Some(name) = v["variableName"].as_str() {
                let cs = v["caseSensitiveLookup"].as_bool().unwrap_or(false);
                let cs_tag = if cs {
                    "case-sensitive"
                } else {
                    "case-insensitive"
                };
                format!("{sl}:{sc}-{el}:{ec} ${name} ({cs_tag})")
            } else if v.get("text").is_some() {
                let text = v["text"].as_str().unwrap_or("");
                format!("{sl}:{sc}-{el}:{ec} text={text:?}")
            } else if v.get("expression").is_some() {
                let expr = v["expression"].as_str().unwrap_or("");
                format!("{sl}:{sc}-{el}:{ec} expr={expr:?}")
            } else {
                format!("{sl}:{sc}-{el}:{ec} <unknown variant>")
            };
            (sl, sc, line)
        })
        .collect();
    rows.sort_by_key(|(sl, sc, _)| (*sl, *sc));
    rows.into_iter()
        .map(|(_, _, s)| s)
        .collect::<Vec<_>>()
        .join("\n")
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
            let sc = l["range"]["start"]["character"].as_u64().unwrap_or(0);
            let el = l["range"]["end"]["line"].as_u64().unwrap_or(0);
            let ec = l["range"]["end"]["character"].as_u64().unwrap_or(0);
            let title = l["command"]["title"].as_str().unwrap_or("<unresolved>");
            let cmd = l["command"]["command"].as_str().unwrap_or("");
            format!("L{sl}:{sc}-L{el}:{ec}: {title} [{cmd}]")
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

/// Decode LSP `semanticTokens/full` response and render each token.
/// LSP encodes tokens as 5-integer sequences: `[deltaLine, deltaStart, length, tokenType, tokenModifiers]`.
/// `legend_types` is the `legend.tokenTypes` array from the initialize response, mapping type indices to names.
pub fn render_semantic_tokens(resp: &Value, legend_types: &[&str]) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let data = resp["result"]["data"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if data.is_empty() {
        return "<no tokens>".to_owned();
    }
    let ints: Vec<u64> = data.iter().map(|v| v.as_u64().unwrap_or(0)).collect();
    if ints.len() % 5 != 0 {
        return format!("<malformed data: {} ints, not a multiple of 5>", ints.len());
    }
    let mut rows = Vec::new();
    let (mut abs_line, mut abs_col) = (0u64, 0u64);
    for chunk in ints.chunks_exact(5) {
        let (dl, dc, len, tt, tm) = (chunk[0], chunk[1], chunk[2], chunk[3], chunk[4]);
        abs_line += dl;
        abs_col = if dl == 0 { abs_col + dc } else { dc };
        let type_name = legend_types.get(tt as usize).copied().unwrap_or("?");
        rows.push(format!(
            "{}:{} len={} type={} mods={:#b}",
            abs_line, abs_col, len, type_name, tm
        ));
    }
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

/// Validate that a span in source code points to a meaningful symbol.
/// Returns error message if span is invalid.
fn validate_span(
    source: &str,
    start_line: u32,
    start_char: u32,
    end_line: u32,
    end_char: u32,
) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();

    // Check bounds
    let start_line = start_line as usize;
    let end_line = end_line as usize;
    if start_line >= lines.len() {
        return Some(format!(
            "span start line {} exceeds source ({} lines)",
            start_line,
            lines.len()
        ));
    }
    if end_line >= lines.len() {
        return Some(format!(
            "span end line {} exceeds source ({} lines)",
            end_line,
            lines.len()
        ));
    }

    // For single-line spans, check the characters
    if start_line == end_line {
        let line = lines[start_line];
        let start_char = start_char as usize;
        let end_char = end_char as usize;

        if end_char > line.len() {
            return Some(format!(
                "span [{}:{}] exceeds line length {} on line {}: {}",
                start_char,
                end_char,
                line.len(),
                start_line,
                line
            ));
        }

        let text = &line[start_char..end_char];

        // Span should start with a symbol (letter, underscore, or digit)
        if !text.chars().next().map_or(false, |c| {
            c.is_alphabetic() || c == '_' || c.is_ascii_digit()
        }) {
            return Some(format!(
                "span [{}:{}] on line {} points to invalid start: {:?} (expected symbol)\nline: {}",
                start_char, end_char, start_line, text, line
            ));
        }
    }

    None
}

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
            .position(|(i, (ap, ar))| !matched[i] && ap == ep && annotation_within_range(er, ar));
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

/// Check whether the annotation caret exactly matches the server's returned
/// range. The annotation start and end columns must equal the actual span's
/// start and end columns on the same line.
fn annotation_within_range(expected: &(u32, u32, u32, u32), actual: &(u32, u32, u32, u32)) -> bool {
    let (esl, esc, _eel, eec) = *expected;
    let (asl, asc, ael, aec) = *actual;
    // Annotation line must fall within the actual range's line span.
    if esl < asl || esl > ael {
        return false;
    }
    // Single-line actual range: require exact start and end column match.
    if asl == ael {
        return esc == asc && eec == aec;
    }
    // Multi-line actual range: exact start-column match on the first line.
    if esl == asl {
        return esc == asc;
    }
    true
}

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
    let actual: Vec<(u32, u32, u32, u32, Option<u32>)> = locs
        .iter()
        .map(|l| {
            let r = &l["range"];
            let kind = l["kind"].as_u64().map(|k| k as u32);
            (
                r["start"]["line"].as_u64().unwrap_or(0) as u32,
                r["start"]["character"].as_u64().unwrap_or(0) as u32,
                r["end"]["line"].as_u64().unwrap_or(0) as u32,
                r["end"]["character"].as_u64().unwrap_or(0) as u32,
                kind,
            )
        })
        .collect();
    let expected_with_tags: Vec<_> = expected
        .iter()
        .filter(|(p, _, _)| p == cursor_path)
        .collect();
    let mut matched = vec![false; actual.len()];
    let mut missing = Vec::new();
    let mut kind_mismatches = Vec::new();
    for (ep, er, tag) in &expected_with_tags {
        let hit = actual
            .iter()
            .enumerate()
            .position(|(i, (al, ac, el, ec, _))| {
                !matched[i] && annotation_within_range(er, &(*al, *ac, *el, *ec))
            });
        match hit {
            Some(i) => {
                matched[i] = true;
                // Validate kind if the tag is "read" or "write"
                let expected_kind = match tag.as_str() {
                    "read" => Some(2),  // DocumentHighlightKind::READ
                    "write" => Some(3), // DocumentHighlightKind::WRITE
                    _ => None,          // "ref" or other tags don't enforce kind
                };
                if let Some(ek) = expected_kind {
                    if actual[i].4 != Some(ek) {
                        kind_mismatches.push((
                            format!("{ep}:{:?}", er),
                            tag.clone(),
                            expected_kind,
                            actual[i].4,
                        ));
                    }
                }
            }
            None => missing.push((ep.clone(), *er, tag.clone())),
        }
    }
    let extras: Vec<_> = actual
        .iter()
        .enumerate()
        .filter(|(i, _)| !matched[*i])
        .map(|(_, (l, c, el, ec, _))| (*l, *c, *el, *ec))
        .collect();
    if !missing.is_empty() || !extras.is_empty() || !kind_mismatches.is_empty() {
        let mut msg = format!(
            "{label} mismatch\nexpected (missing): {missing:#?}\nactual (unmatched): {extras:#?}"
        );
        if !kind_mismatches.is_empty() {
            msg.push_str(&format!("\nkind mismatches: {kind_mismatches:#?}"));
        }
        msg.push_str(&format!("\nfull: {resp}"));
        panic!("{msg}");
    }
}

pub fn lines_of(items: &[Value]) -> Vec<u32> {
    items
        .iter()
        .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
        .collect()
}

// ---------- resolve roundtrip rendering ----------

/// Render a resolved `completionItem/resolve` response as a snapshot-friendly string.
/// Shows label, kind, detail (signature), and documentation content.
pub fn render_resolved_completion_item(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<unresolved>".to_owned();
    }

    let label = result["label"].as_str().unwrap_or("?");
    let kind = completion_kind_name(result["kind"].as_u64().unwrap_or(0));
    let detail = result["detail"]
        .as_str()
        .map(|s| s.trim_end())
        .unwrap_or("<no detail>");
    let docs = result["documentation"]["value"]
        .as_str()
        .or_else(|| result["documentation"].as_str())
        .map(|s| s.trim_end())
        .unwrap_or("<no docs>");

    format!("{label} ({kind})\ndetail: {detail}\ndocs: {docs}")
}

/// Render a resolved `inlayHint/resolve` response as a snapshot-friendly string.
/// Shows position, label, and tooltip content.
pub fn render_resolved_inlay_hint(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<unresolved>".to_owned();
    }

    let line = result["position"]["line"].as_u64().unwrap_or(0);
    let col = result["position"]["character"].as_u64().unwrap_or(0);
    let label = match &result["label"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p["value"].as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };

    let tooltip = result["tooltip"]["value"]
        .as_str()
        .or_else(|| result["tooltip"].as_str())
        .map(|s| s.trim_end())
        .unwrap_or("<no tooltip>");

    format!("{line}:{col} {label}\ntooltip: {tooltip}")
}

/// Render a resolved `workspaceSymbol/resolve` response as a snapshot-friendly string.
/// Shows symbol name, kind, and location with range (or [uri-only] if unresolved).
pub fn render_resolved_workspace_symbol(resp: &Value, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<unresolved>".to_owned();
    }

    let name = result["name"].as_str().unwrap_or("?");
    let kind = symbol_kind_name(result["kind"].as_u64().unwrap_or(0));
    let location = &result["location"];
    let uri = location["uri"].as_str().unwrap_or("?");
    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };
    let short = uri.strip_prefix(&prefix).unwrap_or(uri);

    if let Some(range) = location["range"].as_object() {
        let sl = range["start"]["line"].as_u64().unwrap_or(0);
        let sc = range["start"]["character"].as_u64().unwrap_or(0);
        let el = range["end"]["line"].as_u64().unwrap_or(0);
        let ec = range["end"]["character"].as_u64().unwrap_or(0);
        format!("{name} ({kind}) @ {short}:{sl}:{sc}-{el}:{ec}")
    } else {
        format!("{name} ({kind}) @ {short} [uri-only]")
    }
}

/// Render a `workspace/diagnostic` response as snapshot-friendly text.
/// Format: one file per block, sorted by path. Each diagnostic shows:
/// `L:C message [code] (severity)`. Files with no diagnostics show `<clean>`.
pub fn render_workspace_diagnostic(resp: &Value, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }

    let result = &resp["result"];
    let items = result["items"].as_array().cloned().unwrap_or_default();

    if items.is_empty() {
        return "<no items>".to_owned();
    }

    let prefix = if root_uri.ends_with('/') {
        root_uri.to_owned()
    } else {
        format!("{root_uri}/")
    };

    let mut files: Vec<(String, Vec<String>)> = Vec::new();

    for item in items {
        let uri = item["uri"].as_str().unwrap_or("?");
        let short = uri.strip_prefix(&prefix).unwrap_or(uri).to_owned();

        let diag_lines = if item["kind"].as_str() == Some("unchanged") {
            vec!["  <unchanged>".to_owned()]
        } else {
            // WorkspaceFullDocumentDiagnosticReport has the diagnostics in the "items" field
            // directly (not nested under full_document_diagnostic_report)
            let diags = item["items"].as_array().cloned().unwrap_or_default();

            let mut lines: Vec<String> = diags
                .iter()
                .map(|diag| {
                    let line = diag["range"]["start"]["line"].as_u64().unwrap_or(0);
                    let col = diag["range"]["start"]["character"].as_u64().unwrap_or(0);
                    let msg = diag["message"].as_str().unwrap_or("?");
                    let code = diag["code"]
                        .as_str()
                        .or_else(|| diag["code"].as_u64().map(|_| "0"))
                        .unwrap_or("<unset>");
                    let severity = match diag["severity"].as_u64() {
                        Some(1) => "error",
                        Some(2) => "warning",
                        Some(3) => "info",
                        Some(4) => "hint",
                        _ => "?",
                    };
                    format!("  {line}:{col} {msg} [{code}] ({severity})")
                })
                .collect();

            if lines.is_empty() {
                lines.push("  <clean>".to_owned());
            }
            lines
        };

        files.push((short, diag_lines));
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut out = String::new();
    for (path, diags) in files {
        out.push_str(&format!("{path}\n"));
        for diag in diags {
            out.push_str(&format!("{diag}\n"));
        }
    }

    out.trim_end().to_owned()
}

/// Render a resolved `codeAction/resolve` response as snapshot-friendly text.
/// Shows title, kind, and whether an edit was resolved.
pub fn render_resolved_code_action(resp: &Value, _root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<unresolved>".to_owned();
    }

    let title = result["title"].as_str().unwrap_or("?");
    let kind = result["kind"].as_str().unwrap_or("QuickFix");
    let has_edit = !result["edit"].is_null();

    let edit_summary = if has_edit {
        if let Some(changes) = result["edit"]["changes"].as_object() {
            let file_count = changes.len();
            format!("edit: {file_count} file(s) modified")
        } else {
            "edit: (no changes)".to_owned()
        }
    } else {
        "edit: (unresolved)".to_owned()
    };

    format!("{title} ({kind})\n{edit_summary}")
}

/// Render a resolved `codeLens/resolve` response as snapshot-friendly text.
/// Shows range, command title, and command name.
pub fn render_resolved_code_lens(resp: &Value) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<unresolved>".to_owned();
    }

    let line = result["range"]["start"]["line"].as_u64().unwrap_or(0);
    let char = result["range"]["start"]["character"].as_u64().unwrap_or(0);
    let title = result["command"]["title"].as_str().unwrap_or("?");
    let cmd = result["command"]["command"].as_str().unwrap_or("?");

    format!("L{line}:{char} {title} [{cmd}]")
}

/// Render a resolved `documentLink/resolve` response as snapshot-friendly text.
/// Shows range and target URI.
pub fn render_resolved_document_link(resp: &Value, root_uri: &str) -> String {
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        return format!("error: {err}");
    }
    let result = &resp["result"];
    if result.is_null() {
        return "<unresolved>".to_owned();
    }

    let line = result["range"]["start"]["line"].as_u64().unwrap_or(0);
    let char = result["range"]["start"]["character"].as_u64().unwrap_or(0);
    let target = result["target"].as_str().unwrap_or("?");

    // Strip root prefix if it's a file URI
    let display_target = if target.starts_with("file://") {
        let prefix = if root_uri.ends_with('/') {
            root_uri.to_owned()
        } else {
            format!("{root_uri}/")
        };
        target.strip_prefix(&prefix).unwrap_or(target)
    } else {
        target
    };

    format!("L{line}:{char} -> {display_target}")
}
