//! `resources/views/**` index powering go-to-definition and completion for
//! `view('a.b.c')` calls.
//!
//! Laravel view names are dot-separated paths relative to
//! `resources/views`; `view('admin.dashboard')` resolves to
//! `resources/views/admin/dashboard.blade.php` (or a plain `.php` template).
//! Unlike `env`/`config`, there is no key *inside* the file to point at —
//! the view name resolves to the file itself, so the location is the
//! zero-width start of the template.

use std::collections::HashMap;
use std::path::Path;

use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind, Location, Position, Uri};

use crate::text::zero_width_location;

#[derive(Debug, Default, Clone)]
pub struct ViewIndex {
    views: HashMap<String, Location>,
}

impl ViewIndex {
    pub fn get(&self, name: &str) -> Option<&Location> {
        self.views.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.views.keys().map(String::as_str)
    }

    /// The view name whose template's zero-width start location matches
    /// `uri`/`position`, if any — the reverse of `get`. Since the location
    /// is always `(0, 0)`, this only recognizes the cursor sitting at the
    /// very start of the template file.
    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.views, uri, position)
    }

    pub(super) fn load(root: &Path) -> Self {
        let mut views = HashMap::new();
        let views_dir = root.join("resources").join("views");
        walk(&views_dir, &views_dir, &mut views);
        Self { views }
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
        let Some(name) = view_name_for(base, &path) else {
            continue;
        };
        let Some(uri) = Uri::from_file_path(&path) else {
            continue;
        };
        out.entry(name)
            .or_insert_with(|| zero_width_location(&uri, 0));
    }
}

/// Dot-separated view name for `path`, relative to `base` (the views root).
/// Checks the `.blade.php` suffix before the plain `.php` suffix, since
/// every `.blade.php` file also ends in `.php`.
fn view_name_for(base: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let rel_str = rel.to_str()?.replace('\\', "/");
    let stem = rel_str
        .strip_suffix(".blade.php")
        .or_else(|| rel_str.strip_suffix(".php"))?;
    Some(stem.replace('/', "."))
}

/// Completion items for dot-separated view names starting with `prefix`.
pub(crate) fn view_completions(index: &ViewIndex, prefix: &str) -> Vec<CompletionItem> {
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

    fn write_view(root: &Path, rel: &str, contents: &str) {
        let path = root.join("resources").join("views").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn indexes_top_level_and_nested_blade_views() {
        let tmp = tempfile::tempdir().unwrap();
        write_view(tmp.path(), "welcome.blade.php", "<h1>Hi</h1>");
        write_view(tmp.path(), "admin/dashboard.blade.php", "<h1>Admin</h1>");
        let idx = ViewIndex::load(tmp.path());
        assert!(
            idx.get("welcome")
                .unwrap()
                .uri
                .as_str()
                .ends_with("welcome.blade.php")
        );
        assert!(
            idx.get("admin.dashboard")
                .unwrap()
                .uri
                .as_str()
                .ends_with("admin/dashboard.blade.php")
        );
        assert!(idx.get("nonexistent").is_none());
    }

    #[test]
    fn indexes_plain_php_views() {
        let tmp = tempfile::tempdir().unwrap();
        write_view(tmp.path(), "legacy.php", "<h1>Legacy</h1>");
        let idx = ViewIndex::load(tmp.path());
        assert!(
            idx.get("legacy")
                .unwrap()
                .uri
                .as_str()
                .ends_with("legacy.php")
        );
    }

    #[test]
    fn location_is_zero_width_at_file_start() {
        let tmp = tempfile::tempdir().unwrap();
        write_view(tmp.path(), "welcome.blade.php", "<h1>Hi</h1>");
        let idx = ViewIndex::load(tmp.path());
        let loc = idx.get("welcome").unwrap();
        assert_eq!(loc.range.start.line, 0);
        assert_eq!(loc.range.start, loc.range.end);
        assert!(loc.uri.as_str().ends_with("welcome.blade.php"));
    }

    #[test]
    fn missing_views_dir_yields_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = ViewIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn view_completions_filters_by_prefix_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        write_view(tmp.path(), "admin/dashboard.blade.php", "x");
        write_view(tmp.path(), "admin/users.blade.php", "x");
        write_view(tmp.path(), "welcome.blade.php", "x");
        let idx = ViewIndex::load(tmp.path());
        let items = view_completions(&idx, "admin.");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["admin.dashboard", "admin.users"]);
    }
}
