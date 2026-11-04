//! `app/View/Components/**.php` index powering go-to-definition, hover,
//! document links and completion for class-based Blade components
//! (`<x-alert>` falling back to `App\View\Components\Alert` when no matching
//! `resources/views/components/alert.blade.php` exists — see
//! `blade::resolve_component`).
//!
//! Anonymous components (the common case) resolve directly through
//! `ViewIndex` under the `components.` prefix and need no index of their
//! own; this one only covers the class-based fallback. Keys are kebab-cased
//! and dot-joined per directory segment (`Forms/InputGroup.php` ->
//! `forms.input-group`) to match the tag syntax a template author actually
//! types, so lookups need no case conversion at the call site.

use std::collections::HashMap;
use std::path::Path;

use tower_lsp_server::ls_types::{Location, Position, Uri};

#[derive(Debug, Default, Clone)]
pub struct ComponentIndex {
    components: HashMap<String, Location>,
}

impl ComponentIndex {
    pub fn get(&self, name: &str) -> Option<&Location> {
        self.components.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.components.keys().map(String::as_str)
    }

    pub fn key_at(&self, uri: &Uri, position: Position) -> Option<&str> {
        crate::laravel::location_lookup::key_at(&self.components, uri, position)
    }

    pub(super) fn load(root: &Path) -> Self {
        let mut components = HashMap::new();
        let base = root.join("app").join("View").join("Components");
        super::index_kebab_components(&base, &mut components);
        Self { components }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_component(root: &Path, rel: &str, contents: &str) {
        let path = root.join("app").join("View").join("Components").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn indexes_top_level_and_nested_components_as_kebab() {
        let tmp = tempfile::tempdir().unwrap();
        write_component(tmp.path(), "Alert.php", "<?php class Alert {}\n");
        write_component(
            tmp.path(),
            "Forms/InputGroup.php",
            "<?php class InputGroup {}\n",
        );
        let idx = ComponentIndex::load(tmp.path());
        assert!(
            idx.get("alert")
                .unwrap()
                .uri
                .as_str()
                .ends_with("app/View/Components/Alert.php")
        );
        assert!(
            idx.get("forms.input-group")
                .unwrap()
                .uri
                .as_str()
                .ends_with("app/View/Components/Forms/InputGroup.php")
        );
        assert!(idx.get("nonexistent").is_none());
    }

    #[test]
    fn missing_components_dir_yields_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = ComponentIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }

    #[test]
    fn location_is_zero_width_at_file_start() {
        let tmp = tempfile::tempdir().unwrap();
        write_component(tmp.path(), "Alert.php", "<?php class Alert {}\n");
        let idx = ComponentIndex::load(tmp.path());
        let loc = idx.get("alert").unwrap();
        assert_eq!(loc.range.start, loc.range.end);
        assert!(loc.uri.as_str().ends_with("Alert.php"));
    }
}
