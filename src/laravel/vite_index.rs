//! `public/build/manifest.json` (Vite's manifest) index powering
//! go-to-definition, hover and completion for `vite('resources/js/app.js')`
//! and `Vite::asset('resources/images/logo.svg')` calls.
//!
//! Unlike Laravel Mix's flat manifest, Vite's manifest is keyed by the exact
//! source path passed to `vite()`/`Vite::asset()` (no leading-slash
//! normalization needed) and each entry is an object with (at least) a
//! `file` field — the versioned path under `public/build/`. Only the
//! manifest *key* is indexed; the resolved `file` path is surfaced via the
//! JSON snippet hover already shows, same as `MixIndex`.
//!
//! `vite()` is a bare call, so it reuses `string_call` like every other
//! domain. `Vite::asset()` is a static call and needs its own small
//! detector, in the same shape as `middleware_index`'s `->middleware(...)`
//! collector — except the receiver class *is* checked here (unlike
//! `middleware_index`'s `alias`/`group` or `route_index`'s
//! `resource`/`apiResource`), because "asset" alone isn't Vite-distinctive
//! the way those method names are.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::Path;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{Expr, ExprKind, Span};
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Location, Position, Range, Uri,
};

use crate::document::ast::{ParsedDoc, SourceView};

use super::string_call::content_span;
use super::translation_index::find_json_key_range;

#[derive(Debug, Default, Clone)]
pub struct ViteIndex {
    manifest: HashMap<String, Location>,
}

impl ViteIndex {
    pub fn get(&self, path: &str) -> Option<&Location> {
        self.manifest.get(path)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.manifest.keys().map(String::as_str)
    }

    /// The manifest source path whose declaration contains `position`, if
    /// any — the reverse of `get`, used to recognize a find-references
    /// request starting from the definition site.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.manifest, uri, position)
    }

    pub(super) fn load(root: &Path) -> Self {
        let mut manifest = HashMap::new();
        load_manifest(
            &root.join("public").join("build").join("manifest.json"),
            &mut manifest,
        );
        Self { manifest }
    }
}

fn load_manifest(path: &Path, out: &mut HashMap<String, Location>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(map) = json.as_object() else {
        return;
    };
    let Some(uri) = Uri::from_file_path(path) else {
        return;
    };
    for key in map.keys() {
        if out.contains_key(key.as_str()) {
            continue;
        }
        if let Some(range) = find_json_key_range(&text, key) {
            out.insert(
                key.clone(),
                Location {
                    uri: uri.clone(),
                    range,
                },
            );
        }
    }
}

/// Completion items for Vite manifest source paths starting with `prefix`.
pub(crate) fn vite_completions(index: &ViteIndex, prefix: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = index
        .names()
        .filter(|name| name.starts_with(prefix))
        .map(|name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FILE),
            insert_text: Some(name.to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

fn is_ident(expr: &Expr<'_, '_>, name: &str) -> bool {
    matches!(&expr.kind, ExprKind::Identifier(n) if n.eq_ignore_ascii_case(name))
}

/// One `Vite::asset('...')` call site found in application code.
/// `token_span` is the full string literal's span (quotes included) — used
/// for cursor-containment — while `range` covers only the path text.
struct ViteAssetCall {
    token_span: Span,
    path: String,
    range: Range,
}

fn collect_vite_asset_calls(doc: &ParsedDoc) -> Vec<ViteAssetCall> {
    struct Collector<'a> {
        source: &'a str,
        sv: SourceView<'a>,
        out: Vec<ViteAssetCall>,
    }

    impl<'arena, 'src> Visitor<'arena, 'src> for Collector<'_> {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if let ExprKind::StaticMethodCall(s) = &expr.kind
                && is_ident(s.class, "Vite")
                && is_ident(s.method, "asset")
                && let Some(arg) = s.args.first()
                && let Some(arg_value) = &arg.value
                && let ExprKind::String(path) = &arg_value.kind
                && let Some(cspan) = content_span(self.source, arg_value.span)
            {
                self.out.push(ViteAssetCall {
                    token_span: arg_value.span,
                    path: (*path).to_string(),
                    range: self.sv.range_of(cspan),
                });
            }
            walk_expr(self, expr)
        }
    }

    let mut collector = Collector {
        source: doc.source(),
        sv: doc.view(),
        out: Vec::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = collector.visit_stmt(stmt);
    }
    collector.out
}

/// The Vite asset path and its `Range`, when the cursor sits inside a
/// `Vite::asset('...')` call's string argument.
pub(crate) fn vite_asset_at(doc: &ParsedDoc, position: Position) -> Option<(String, Range)> {
    let sv = doc.view();
    let offset = sv.byte_of_position(position);
    collect_vite_asset_calls(doc)
        .into_iter()
        .find(|c| c.token_span.start <= offset && offset < c.token_span.end)
        .map(|c| (c.path, c.range))
}

/// Every `Vite::asset(...)` usage in `doc`, decoded and with its `Range` —
/// used to build document links for a whole file in one AST walk.
pub(crate) fn collect_vite_asset_links(doc: &ParsedDoc) -> Vec<(String, Range)> {
    collect_vite_asset_calls(doc)
        .into_iter()
        .map(|c| (c.path, c.range))
        .collect()
}

/// Typed prefix (from the opening quote up to the cursor), when the cursor
/// sits inside a string literal — closed or not — immediately following
/// `Vite::asset(`. Used for completion, where the closing quote may not
/// exist yet. Only the single-string-argument form is recognized — same
/// scope as every other bare-call domain's `*_string_prefix` helper.
pub(crate) fn vite_asset_string_prefix(source: &str, position: Position) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let line = *lines.get(position.line as usize)?;
    let byte_col = crate::text::utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..byte_col];
    let quote_pos = before.rfind(['\'', '"'])?;
    let head = before[..quote_pos].trim_end().strip_suffix('(')?.trim_end();
    let head = strip_suffix_word(head, "asset")?.trim_end();
    let head = head.strip_suffix("::")?;
    if !ends_with_word(head, "Vite") {
        return None;
    }
    Some(before[quote_pos + 1..].to_string())
}

/// `s` with `word` removed from the end, if `s` ends with `word` at a word
/// boundary (so e.g. `myasset` doesn't match `asset`).
fn strip_suffix_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    ends_with_word(s, word).then(|| &s[..s.len() - word.len()])
}

fn ends_with_word(s: &str, word: &str) -> bool {
    s.len() >= word.len()
        && s[s.len() - word.len()..].eq_ignore_ascii_case(word)
        && word_boundary_before(s, s.len() - word.len())
}

fn word_boundary_before(s: &str, idx: usize) -> bool {
    idx == 0 || !matches!(s.as_bytes()[idx - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Position;

    fn write_manifest(root: &Path, contents: &str) {
        let build = root.join("public").join("build");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("manifest.json"), contents).unwrap();
    }

    fn parse(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    #[test]
    fn resolves_source_path_key() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r#"{"resources/js/app.js": {"file": "assets/app-4ed993c7.js"}}"#,
        );
        let idx = ViteIndex::load(tmp.path());
        let loc = idx.get("resources/js/app.js").unwrap();
        assert!(loc.uri.as_str().ends_with("build/manifest.json"));
    }

    #[test]
    fn unknown_path_resolves_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r#"{"resources/js/app.js": {"file": "assets/app-4ed993c7.js"}}"#,
        );
        let idx = ViteIndex::load(tmp.path());
        assert!(idx.get("resources/js/missing.js").is_none());
    }

    #[test]
    fn missing_manifest_yields_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = ViteIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn vite_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r#"{"resources/css/app.css": {"file": "a"}, "resources/css/admin.css": {"file": "b"}, "resources/js/app.js": {"file": "c"}}"#,
        );
        let idx = ViteIndex::load(tmp.path());
        let items = vite_completions(&idx, "resources/css/");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["resources/css/admin.css", "resources/css/app.css"]
        );
    }

    #[test]
    fn vite_asset_at_matches_static_call() {
        let doc = parse("<?php\necho Vite::asset('resources/images/logo.svg');\n");
        let pos = Position {
            line: 1,
            character: 25,
        };
        let (path, _) = vite_asset_at(&doc, pos).unwrap();
        assert_eq!(path, "resources/images/logo.svg");
    }

    #[test]
    fn vite_asset_at_rejects_unrelated_static_call() {
        let doc = parse("<?php\necho Storage::asset('logo.svg');\n");
        let pos = Position {
            line: 1,
            character: 25,
        };
        assert!(vite_asset_at(&doc, pos).is_none());
    }

    #[test]
    fn collect_vite_asset_links_finds_every_site() {
        let doc = parse("<?php\n$a = Vite::asset('a.svg');\n$b = Vite::asset('b.svg');\n");
        let links = collect_vite_asset_links(&doc);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].0, "a.svg");
        assert_eq!(links[1].0, "b.svg");
    }

    #[test]
    fn vite_asset_string_prefix_matches_static_call() {
        let src = "<?php\necho Vite::asset('resources/images/lo";
        let pos = Position {
            line: 1,
            character: 37,
        };
        assert_eq!(
            vite_asset_string_prefix(src, pos).as_deref(),
            Some("resources/images/lo")
        );
    }

    #[test]
    fn vite_asset_string_prefix_rejects_unrelated_call() {
        let src = "<?php\necho Storage::asset('lo";
        let pos = Position {
            line: 1,
            character: 23,
        };
        assert!(vite_asset_string_prefix(src, pos).is_none());
    }
}
