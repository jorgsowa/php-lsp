//! `lang/**` (and legacy `resources/lang/**`) index powering go-to-definition
//! and completion for `__('a.b')` / `trans('a.b')` calls, plus the
//! literal-string JSON translation convention (`__('Original string')`).
//!
//! Laravel's current default location is `lang/{locale}/*.php` +
//! `lang/{locale}.json` at the project root; older/legacy projects (and
//! some packages) keep the same layout under `resources/lang/`. Both roots
//! are scanned — `lang/` first, so it wins a key collision. Within each
//! root, the `en` locale is visited before others (then alphabetically), so
//! the first locale to define a key wins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use php_ast::{ArrayElement, ExprKind, StmtKind};
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Location, Position, Range, Uri,
};

use crate::analysis::diagnostics::parse_document_no_diags;
use crate::document::ast::{SourceView, offset_to_position};

#[derive(Debug, Default, Clone)]
pub struct TranslationIndex {
    keys: HashMap<String, Location>,
}

impl TranslationIndex {
    pub fn get(&self, key: &str) -> Option<&Location> {
        self.keys.get(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.keys().map(String::as_str)
    }

    /// The translation key whose declaration contains `position`, if any —
    /// the reverse of `get`, used to recognize a find-references request
    /// starting from the definition site.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.keys, uri, position)
    }

    pub(super) fn load(root: &Path) -> Self {
        let mut keys = HashMap::new();
        for lang_root in [root.join("lang"), root.join("resources").join("lang")] {
            load_lang_root(&lang_root, &mut keys);
        }
        Self { keys }
    }
}

fn load_lang_root(lang_root: &Path, out: &mut HashMap<String, Location>) {
    let Ok(entries) = std::fs::read_dir(lang_root) else {
        return;
    };
    let mut locale_dirs: Vec<PathBuf> = Vec::new();
    let mut json_files: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            locale_dirs.push(path);
        } else if path.extension().is_some_and(|e| e == "json") {
            json_files.push(path);
        }
    }
    // "en" first, then alphabetical — the first locale to define a key wins.
    locale_dirs.sort_by_key(|p| {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        (name != "en", name)
    });
    for dir in &locale_dirs {
        load_locale_php_files(dir, out);
    }
    json_files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    for file in &json_files {
        load_locale_json_file(file, out);
    }
}

/// Direct `.php` children of one locale directory (e.g. `lang/en/`) —
/// nested subdirectories are a known gap, matching `ConfigIndex`'s same
/// simplification for `config/`.
fn load_locale_php_files(locale_dir: &Path, out: &mut HashMap<String, Location>) {
    let Ok(entries) = std::fs::read_dir(locale_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "php") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Some(uri) = Uri::from_file_path(&path) else {
            continue;
        };
        let doc = parse_document_no_diags(&text);
        let sv = doc.view();
        for stmt in doc.program().stmts.iter() {
            if let StmtKind::Return(Some(expr)) = &stmt.kind
                && let ExprKind::Array(elements) = &expr.kind
            {
                collect_array_keys(elements, sv, &uri, stem, out);
            }
        }
    }
}

fn collect_array_keys(
    elements: &[ArrayElement<'_, '_>],
    sv: SourceView<'_>,
    uri: &Uri,
    prefix: &str,
    out: &mut HashMap<String, Location>,
) {
    for el in elements {
        let Some(key_expr) = &el.key else { continue };
        let ExprKind::String(key) = &key_expr.kind else {
            continue;
        };
        let dotted = format!("{prefix}.{key}");
        // `span.start`/`span.end` point at the surrounding quotes (see
        // `editing::document_link::link_from_path_expr`); trim one byte off
        // each side to land on the key text itself.
        let range = Range {
            start: sv.position_of(key_expr.span.start + 1),
            end: sv.position_of(key_expr.span.end - 1),
        };
        out.entry(dotted.clone()).or_insert_with(|| Location {
            uri: uri.clone(),
            range,
        });
        if let ExprKind::Array(nested) = &el.value.kind {
            collect_array_keys(nested, sv, uri, &dotted, out);
        }
    }
}

/// `lang/{locale}.json` — a flat map from the literal default-language
/// string to its translation. The map *key itself* (not a dotted path) is
/// what `__('...')` is called with.
fn load_locale_json_file(path: &Path, out: &mut HashMap<String, Location>) {
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

/// Locate `key`'s first `"key"` occurrence in raw JSON text and return the
/// range of the key text itself (quotes excluded). Manual text search
/// because `serde_json::Value` doesn't retain source spans.
fn find_json_key_range(text: &str, key: &str) -> Option<Range> {
    let escaped = key.replace('\\', "\\\\").replace('"', "\\\"");
    let needle = format!("\"{escaped}\"");
    let quote_byte = text.find(&needle)?;
    let byte_start = (quote_byte + 1) as u32;
    let byte_end = byte_start + escaped.len() as u32;
    let line_starts = line_starts_of(text);
    Some(Range {
        start: offset_to_position(text, &line_starts, byte_start),
        end: offset_to_position(text, &line_starts, byte_end),
    })
}

fn line_starts_of(text: &str) -> Vec<u32> {
    std::iter::once(0u32)
        .chain(text.match_indices('\n').map(|(i, _)| i as u32 + 1))
        .collect()
}

/// Completion items for translation keys starting with `prefix`.
pub(crate) fn translation_completions(
    index: &TranslationIndex,
    prefix: &str,
) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = index
        .keys()
        .filter(|key| key.starts_with(prefix))
        .map(|key| CompletionItem {
            label: key.to_string(),
            kind: Some(CompletionItemKind::TEXT),
            insert_text: Some(key.to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_php_lang(root: &Path, lang_dir: &str, locale: &str, name: &str, contents: &str) {
        let dir = root.join(lang_dir).join(locale);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), contents).unwrap();
    }

    #[test]
    fn indexes_nested_php_keys_under_lang() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn [\n    'failed' => 'These credentials do not match.',\n];\n",
        );
        let idx = TranslationIndex::load(tmp.path());
        assert!(idx.get("auth.failed").is_some());
    }

    #[test]
    fn prefers_en_locale_on_key_collision() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "es",
            "auth.php",
            "<?php\nreturn ['failed' => 'Spanish'];\n",
        );
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'English'];\n",
        );
        let idx = TranslationIndex::load(tmp.path());
        let loc = idx.get("auth.failed").unwrap();
        assert!(loc.uri.as_str().contains("/en/"));
    }

    #[test]
    fn prefers_root_lang_over_resources_lang() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "resources/lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'Legacy'];\n",
        );
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn ['failed' => 'Current'];\n",
        );
        let idx = TranslationIndex::load(tmp.path());
        let loc = idx.get("auth.failed").unwrap();
        assert!(!loc.uri.as_str().contains("resources"));
    }

    #[test]
    fn indexes_json_literal_translation_keys() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("lang")).unwrap();
        std::fs::write(
            tmp.path().join("lang").join("es.json"),
            r#"{"I love programming.": "Me encanta programar."}"#,
        )
        .unwrap();
        let idx = TranslationIndex::load(tmp.path());
        let loc = idx.get("I love programming.").unwrap();
        assert!(loc.uri.as_str().ends_with("es.json"));
        assert_eq!(loc.range.start.line, 0);
    }

    #[test]
    fn missing_lang_dirs_yield_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = TranslationIndex::load(tmp.path());
        assert_eq!(idx.keys().count(), 0);
    }

    #[test]
    fn translation_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write_php_lang(
            tmp.path(),
            "lang",
            "en",
            "auth.php",
            "<?php\nreturn [\n    'failed' => 'x',\n    'throttle' => 'y',\n];\n",
        );
        let idx = TranslationIndex::load(tmp.path());
        let items = translation_completions(&idx, "auth.");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["auth.failed", "auth.throttle"]);
    }
}
