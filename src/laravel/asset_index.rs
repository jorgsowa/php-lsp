//! `public/**` index powering go-to-definition and completion for
//! `asset('path/to/file')` calls.
//!
//! Unlike view names, asset paths keep their real relative path (including
//! extension) rather than being converted to dot notation — `asset('css/app.css')`
//! resolves to `public/css/app.css` verbatim. There is no key *inside* the
//! file to point at, so — like `ViewIndex` — the location is the zero-width
//! start of the asset file itself.

use std::collections::HashMap;
use std::path::Path;

use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind, Location, Position, Uri};

use crate::text::zero_width_location;

#[derive(Debug, Default, Clone)]
pub struct AssetIndex {
    assets: HashMap<String, Location>,
}

impl AssetIndex {
    pub fn get(&self, path: &str) -> Option<&Location> {
        self.assets.get(path)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.assets.keys().map(String::as_str)
    }

    /// The asset path whose file's zero-width start location matches
    /// `uri`/`position`, if any — the reverse of `get`, used to recognize a
    /// find-references request starting from the definition site.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.assets, uri, position)
    }

    pub(super) fn load(root: &Path) -> Self {
        let mut assets = HashMap::new();
        let public_dir = root.join("public");
        walk(&public_dir, &public_dir, &mut assets);
        Self { assets }
    }
}

fn walk(base: &Path, dir: &Path, out: &mut HashMap<String, Location>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            walk(base, &path, out);
            continue;
        }
        let Some(rel) = path.strip_prefix(base).ok().and_then(|r| r.to_str()) else {
            continue;
        };
        let rel = rel.replace('\\', "/");
        let Some(uri) = Uri::from_file_path(&path) else {
            continue;
        };
        out.entry(rel)
            .or_insert_with(|| zero_width_location(&uri, 0));
    }
}

/// Completion items for asset paths starting with `prefix`.
pub(crate) fn asset_completions(index: &AssetIndex, prefix: &str) -> Vec<CompletionItem> {
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

    fn write_asset(root: &Path, rel: &str, contents: &str) {
        let path = root.join("public").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn indexes_top_level_and_nested_assets() {
        let tmp = tempfile::tempdir().unwrap();
        write_asset(tmp.path(), "favicon.ico", "x");
        write_asset(tmp.path(), "css/app.css", "body{}");
        let idx = AssetIndex::load(tmp.path());
        assert!(idx.get("favicon.ico").is_some());
        assert!(idx.get("css/app.css").is_some());
        assert!(idx.get("nonexistent.js").is_none());
    }

    #[test]
    fn location_is_zero_width_at_file_start() {
        let tmp = tempfile::tempdir().unwrap();
        write_asset(tmp.path(), "css/app.css", "body{}");
        let idx = AssetIndex::load(tmp.path());
        let loc = idx.get("css/app.css").unwrap();
        assert_eq!(loc.range.start, loc.range.end);
        assert!(loc.uri.as_str().ends_with("css/app.css"));
    }

    #[test]
    fn missing_public_dir_yields_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = AssetIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn asset_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write_asset(tmp.path(), "css/app.css", "x");
        write_asset(tmp.path(), "css/admin.css", "x");
        write_asset(tmp.path(), "js/app.js", "x");
        let idx = AssetIndex::load(tmp.path());
        let items = asset_completions(&idx, "css/");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["css/admin.css", "css/app.css"]);
    }
}
