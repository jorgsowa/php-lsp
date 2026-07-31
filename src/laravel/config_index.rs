//! `config/*.php` index powering go-to-definition and completion for
//! `config('a.b.c')` calls.
//!
//! Each file under `config/` (direct children only — matches Laravel's
//! standard layout; nested config directories are a known gap) is expected
//! to `return [...]` a — possibly nested — associative array. Every
//! string-keyed entry becomes `file_stem.path.to.key -> Location`, indexed
//! at every nesting level so both `config('database')` (the whole file) and
//! `config('database.connections.mysql.host')` (a leaf) resolve.

use std::collections::HashMap;
use std::path::Path;

use php_ast::{ArrayElement, ExprKind, StmtKind};
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Location, Position, Range, Uri,
};

use crate::analysis::diagnostics::parse_document_no_diags;
use crate::document::ast::SourceView;

#[derive(Debug, Default, Clone)]
pub struct ConfigIndex {
    keys: HashMap<String, Location>,
}

impl ConfigIndex {
    pub fn get(&self, key: &str) -> Option<&Location> {
        self.keys.get(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.keys().map(String::as_str)
    }

    /// The dotted config key whose declaration in `config/*.php` contains
    /// `position`, if any — the reverse of `get`, used to recognize a
    /// find-references request starting from the definition site.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.keys, uri, position)
    }

    pub(super) fn load(root: &Path) -> Self {
        let mut keys = HashMap::new();
        let Ok(entries) = std::fs::read_dir(root.join("config")) else {
            return Self { keys };
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
                    collect_array_keys(elements, sv, &uri, stem, &mut keys);
                }
            }
        }
        Self { keys }
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

/// Completion items for dotted config keys starting with `prefix`.
pub(crate) fn config_completions(index: &ConfigIndex, prefix: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = index
        .keys()
        .filter(|key| key.starts_with(prefix))
        .map(|key| CompletionItem {
            label: key.to_string(),
            kind: Some(CompletionItemKind::PROPERTY),
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

    fn write_config(root: &Path, name: &str, contents: &str) {
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(root.join("config").join(name), contents).unwrap();
    }

    #[test]
    fn indexes_top_level_and_nested_keys() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(
            tmp.path(),
            "database.php",
            "<?php\nreturn [\n    'default' => 'mysql',\n    'connections' => [\n        'mysql' => [\n            'host' => '127.0.0.1',\n        ],\n    ],\n];\n",
        );
        let idx = ConfigIndex::load(tmp.path());
        assert!(idx.get("database.default").is_some());
        assert!(idx.get("database.connections").is_some());
        assert!(idx.get("database.connections.mysql").is_some());
        assert!(idx.get("database.connections.mysql.host").is_some());
        assert!(idx.get("database.nonexistent").is_none());
    }

    #[test]
    fn range_excludes_surrounding_quotes() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(
            tmp.path(),
            "app.php",
            "<?php\nreturn [\n    'name' => 'Laravel',\n];\n",
        );
        let idx = ConfigIndex::load(tmp.path());
        let loc = idx.get("app.name").unwrap();
        // Line 2 (0-based): `    'name' => 'Laravel',` — "name" spans cols 5..9.
        assert_eq!(loc.range.start.line, 2);
        assert_eq!(loc.range.start.character, 5);
        assert_eq!(loc.range.end.character, 9);
    }

    #[test]
    fn ignores_non_php_files_and_missing_config_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = ConfigIndex::load(tmp.path());
        assert_eq!(idx.keys().count(), 0);

        write_config(tmp.path(), "readme.md", "not php");
        let idx = ConfigIndex::load(tmp.path());
        assert_eq!(idx.keys().count(), 0);
    }

    #[test]
    fn skips_positional_and_non_string_keys() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(
            tmp.path(),
            "app.php",
            "<?php\nreturn [\n    'providers' => [\n        Foo::class,\n        Bar::class,\n    ],\n];\n",
        );
        let idx = ConfigIndex::load(tmp.path());
        assert!(idx.get("app.providers").is_some());
        // Positional list entries (no string key) contribute nothing further.
        assert_eq!(idx.keys().count(), 1);
    }

    #[test]
    fn config_completions_filters_by_dotted_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write_config(
            tmp.path(),
            "app.php",
            "<?php\nreturn [\n    'name' => 'x',\n    'env' => 'y',\n];\n",
        );
        let idx = ConfigIndex::load(tmp.path());
        let items = config_completions(&idx, "app.");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["app.env", "app.name"]);
    }
}
