//! `public/mix-manifest.json` index powering go-to-definition, hover and
//! completion for `mix('path/to/asset.js')` calls.
//!
//! The manifest is a flat map from a leading-slash source path to its
//! versioned output path (e.g. `"/js/app.js": "/js/app.js?id=abcdef123456"`).
//! `mix()` is called without the leading slash, so it's stripped on load —
//! same shape as `TranslationIndex`'s `lang/{locale}.json` handling, just a
//! different manifest schema.

use std::collections::HashMap;
use std::path::Path;

use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind, Location, Position, Uri};

use super::translation_index::find_json_key_range;

#[derive(Debug, Default, Clone)]
pub struct MixIndex {
    manifest: HashMap<String, Location>,
}

impl MixIndex {
    pub fn get(&self, path: &str) -> Option<&Location> {
        self.manifest.get(path.strip_prefix('/').unwrap_or(path))
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
            &root.join("public").join("mix-manifest.json"),
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
        let bare = key.strip_prefix('/').unwrap_or(key);
        if out.contains_key(bare) {
            continue;
        }
        if let Some(range) = find_json_key_range(&text, key) {
            out.insert(
                bare.to_string(),
                Location {
                    uri: uri.clone(),
                    range,
                },
            );
        }
    }
}

/// Completion items for mix manifest source paths starting with `prefix`.
pub(crate) fn mix_completions(index: &MixIndex, prefix: &str) -> Vec<CompletionItem> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(root: &Path, contents: &str) {
        let public = root.join("public");
        std::fs::create_dir_all(&public).unwrap();
        std::fs::write(public.join("mix-manifest.json"), contents).unwrap();
    }

    #[test]
    fn resolves_bare_path_against_leading_slash_key() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r#"{"/js/app.js": "/js/app.js?id=abc123"}"#,
        );
        let idx = MixIndex::load(tmp.path());
        let loc = idx.get("js/app.js").unwrap();
        assert!(loc.uri.as_str().ends_with("mix-manifest.json"));
        assert_eq!(idx.get("/js/app.js").unwrap().range, loc.range);
    }

    #[test]
    fn location_points_at_the_manifest_key_text() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r#"{"/js/app.js": "/js/app.js?id=abc123"}"#,
        );
        let idx = MixIndex::load(tmp.path());
        let loc = idx.get("js/app.js").unwrap();
        assert_eq!((loc.range.start.line, loc.range.start.character), (0, 2));
        assert_eq!(loc.range.end.character, 12);
    }

    #[test]
    fn unknown_path_resolves_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), r#"{"/js/app.js": "/js/app.js"}"#);
        let idx = MixIndex::load(tmp.path());
        assert!(idx.get("js/missing.js").is_none());
    }

    #[test]
    fn missing_manifest_yields_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = MixIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn mix_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(
            tmp.path(),
            r#"{"/css/app.css": "/css/app.css?id=1", "/css/admin.css": "/css/admin.css?id=2", "/js/app.js": "/js/app.js?id=3"}"#,
        );
        let idx = MixIndex::load(tmp.path());
        let items = mix_completions(&idx, "css/");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["css/admin.css", "css/app.css"]);
    }
}
