//! `.env` / `.env.example` index powering go-to-definition and completion for
//! `env('KEY')` calls.

use std::collections::HashMap;
use std::path::Path;

use tower_lsp_server::ls_types::{
    CompletionItem, CompletionItemKind, Location, Position, Range, Uri,
};

#[derive(Debug, Default, Clone)]
pub struct EnvIndex {
    vars: HashMap<String, Location>,
}

impl EnvIndex {
    pub fn get(&self, name: &str) -> Option<&Location> {
        self.vars.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.vars.keys().map(String::as_str)
    }

    /// The env var name whose `.env`/`.env.example` declaration contains
    /// `position`, if any — the reverse of `get`, used to recognize a
    /// find-references request starting from the definition site.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.vars, uri, position)
    }

    /// Scan `.env` then `.env.example` at `root`. A key already found in
    /// `.env` is not overwritten by `.env.example` — the real file (when
    /// present) reflects the developer's actual configuration, and is more
    /// likely to be the file they expect a jump-to-definition to land in.
    pub(super) fn load(root: &Path) -> Self {
        let mut vars = HashMap::new();
        for filename in [".env", ".env.example"] {
            let path = root.join(filename);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(uri) = Uri::from_file_path(&path) else {
                continue;
            };
            for (line_no, line) in text.lines().enumerate() {
                let Some((key, start, end)) = parse_env_key(line) else {
                    continue;
                };
                vars.entry(key.to_string()).or_insert_with(|| Location {
                    uri: uri.clone(),
                    range: Range {
                        start: Position {
                            line: line_no as u32,
                            character: start as u32,
                        },
                        end: Position {
                            line: line_no as u32,
                            character: end as u32,
                        },
                    },
                });
            }
        }
        Self { vars }
    }
}

/// Parse a `.env` line into `(key, key_start_col, key_end_col)`. Skips blank
/// lines, comments (`#`), and lines without a valid identifier before `=`.
/// Columns are plain char counts — `.env` files are conventionally ASCII.
fn parse_env_key(line: &str) -> Option<(&str, usize, usize)> {
    let trimmed_start = line.trim_start();
    let leading_ws = line.len() - trimmed_start.len();
    if trimmed_start.is_empty() || trimmed_start.starts_with('#') {
        return None;
    }
    let rest = trimmed_start
        .strip_prefix("export ")
        .unwrap_or(trimmed_start);
    let export_prefix_len = trimmed_start.len() - rest.len();
    let eq_pos = rest.find('=')?;
    let key = rest[..eq_pos].trim_end();
    if key.is_empty()
        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || key.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return None;
    }
    let key_start = leading_ws + export_prefix_len;
    Some((key, key_start, key_start + key.len()))
}

/// Completion items for env var names starting with `prefix`.
pub(crate) fn env_completions(index: &EnvIndex, prefix: &str) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = index
        .names()
        .filter(|name| name.starts_with(prefix))
        .map(|name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::CONSTANT),
            insert_text: Some(name.to_string()),
            ..Default::default()
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_env_key_simple() {
        assert_eq!(parse_env_key("APP_NAME=Laravel"), Some(("APP_NAME", 0, 8)));
    }

    #[test]
    fn parse_env_key_skips_comment() {
        assert_eq!(parse_env_key("# a comment"), None);
    }

    #[test]
    fn parse_env_key_skips_blank() {
        assert_eq!(parse_env_key("   "), None);
        assert_eq!(parse_env_key(""), None);
    }

    #[test]
    fn parse_env_key_handles_export_prefix() {
        assert_eq!(
            parse_env_key("export DB_HOST=127.0.0.1"),
            Some(("DB_HOST", 7, 14))
        );
    }

    #[test]
    fn parse_env_key_handles_leading_whitespace() {
        assert_eq!(
            parse_env_key("  APP_KEY=base64:xyz"),
            Some(("APP_KEY", 2, 9))
        );
    }

    #[test]
    fn parse_env_key_rejects_leading_digit() {
        assert_eq!(parse_env_key("1FOO=bar"), None);
    }

    #[test]
    fn parse_env_key_rejects_no_equals() {
        assert_eq!(parse_env_key("just some text"), None);
    }

    #[test]
    fn parse_env_key_rejects_non_identifier() {
        assert_eq!(parse_env_key("FOO-BAR=baz"), None);
    }

    #[test]
    fn load_prefers_env_over_example() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".env"), "APP_NAME=Real\n").unwrap();
        std::fs::write(
            tmp.path().join(".env.example"),
            "APP_NAME=Example\nAPP_ENV=local\n",
        )
        .unwrap();
        let idx = EnvIndex::load(tmp.path());
        let real = idx.get("APP_NAME").unwrap();
        assert!(real.uri.as_str().ends_with("/.env"));
        assert!(
            idx.get("APP_ENV")
                .unwrap()
                .uri
                .as_str()
                .ends_with(".env.example")
        );
    }

    #[test]
    fn load_missing_files_yields_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = EnvIndex::load(tmp.path());
        assert!(idx.get("ANYTHING").is_none());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn env_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".env"),
            "APP_NAME=x\nAPP_ENV=y\nDB_HOST=z\n",
        )
        .unwrap();
        let idx = EnvIndex::load(tmp.path());
        let items = env_completions(&idx, "APP_");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["APP_ENV", "APP_NAME"]);
    }
}
