//! `config/**/*.php` index powering go-to-definition and completion for
//! `config('a.b.c')` calls.
//!
//! Every file under `config/`, including nested subdirectories, is expected
//! to `return [...]` a — possibly nested — associative array. A file's
//! prefix is its path relative to `config/` with the `.php` extension
//! stripped and directory separators replaced by `.` (so
//! `config/services/stripe.php` becomes prefix `services.stripe`). Every
//! string-keyed entry becomes `prefix.path.to.key -> Location`, indexed at
//! every nesting level so both `config('database')` (the whole file) and
//! `config('database.connections.mysql.host')` (a leaf) resolve.

use std::collections::HashMap;
use std::path::Path;

use php_ast::{ExprKind, StmtKind};
use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Location, Position, Range, Uri,
};

use crate::analysis::diagnostics::parse_document_no_diags;

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
        let config_dir = root.join("config");
        for path in super::fs_walk::php_files_recursive(&config_dir) {
            let Some(prefix) = super::relative_key(&config_dir, &path, ".") else {
                continue;
            };
            let Ok(text) = std::fs::read(&path).map(crate::text::decode_source) else {
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
                    // The whole-file key points at the `return` keyword.
                    keys.entry(prefix.clone()).or_insert_with(|| Location {
                        uri: uri.clone(),
                        range: Range {
                            start: sv.position_of(stmt.span.start),
                            end: sv.position_of(stmt.span.start + "return".len() as u32),
                        },
                    });
                    super::collect_array_keys(elements, sv, &uri, &prefix, &mut keys);
                }
            }
        }
        Self { keys }
    }
}

/// Completion items for dotted config keys starting with `prefix`.
pub(crate) fn config_completions(index: &ConfigIndex, prefix: &str) -> Vec<CompletionItem> {
    super::prefix_completions(index.keys(), prefix, CompletionItemKind::PROPERTY)
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

        let default = idx.get("database.default").unwrap();
        assert_eq!(
            (default.range.start.line, default.range.start.character),
            (2, 5)
        );
        assert_eq!(default.range.end.character, 12);

        let connections = idx.get("database.connections").unwrap();
        assert_eq!(
            (
                connections.range.start.line,
                connections.range.start.character
            ),
            (3, 5)
        );
        assert_eq!(connections.range.end.character, 16);

        let mysql = idx.get("database.connections.mysql").unwrap();
        assert_eq!(
            (mysql.range.start.line, mysql.range.start.character),
            (4, 9)
        );
        assert_eq!(mysql.range.end.character, 14);

        let host = idx.get("database.connections.mysql.host").unwrap();
        assert_eq!((host.range.start.line, host.range.start.character), (5, 13));
        assert_eq!(host.range.end.character, 17);

        assert!(idx.get("database.nonexistent").is_none());

        let file = idx.get("database").unwrap();
        assert_eq!((file.range.start.line, file.range.start.character), (1, 0));
        assert_eq!(file.range.end.character, 6);
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
    fn indexes_nested_config_directory() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("config").join("services")).unwrap();
        std::fs::write(
            tmp.path()
                .join("config")
                .join("services")
                .join("stripe.php"),
            "<?php\nreturn [\n    'key' => 'abc',\n];\n",
        )
        .unwrap();
        let idx = ConfigIndex::load(tmp.path());
        let loc = idx.get("services.stripe.key").unwrap();
        assert!(loc.uri.as_str().ends_with("config/services/stripe.php"));
        assert!(idx.get("services.stripe").is_some());
    }

    #[test]
    fn indexes_config_file_that_is_not_utf8() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("config")).unwrap();
        std::fs::write(
            tmp.path().join("config").join("app.php"),
            b"<?php\n// caf\xe9\nreturn [\n    'name' => 'Caf\xe9',\n];\n",
        )
        .unwrap();
        let idx = ConfigIndex::load(tmp.path());
        let loc = idx.get("app.name").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (3, 5));
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
        let providers = idx.get("app.providers").unwrap();
        assert_eq!(
            (providers.range.start.line, providers.range.start.character),
            (2, 5)
        );
        assert_eq!(providers.range.end.character, 14);
        // Positional list entries (no string key) contribute nothing further.
        assert_eq!(idx.keys().count(), 2);
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
        let items = config_completions(&idx, "app");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["app", "app.env", "app.name"]);
    }
}
