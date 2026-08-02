//! Livewire component class index powering go-to-definition, hover,
//! document links and completion for `<livewire:counter>` tags and
//! `@livewire('counter')` directives.
//!
//! Both the Livewire 3 convention (`app/Livewire/**.php`) and the legacy
//! Livewire 2 one (`app/Http/Livewire/**.php`) are scanned unconditionally —
//! a project only ever has one or the other, mirroring `MiddlewareIndex`'s
//! `bootstrap/app.php`/`Kernel.php` pair. Keys are kebab-dotted per directory
//! segment (`Forms/Counter.php` -> `forms.counter`), matching the tag/
//! directive syntax directly, same convention as `ComponentIndex`.
//! View-only Livewire components (no class, just
//! `resources/views/livewire/*.blade.php`) fall back through `ViewIndex`
//! under the `livewire.` prefix — see `blade::resolve_livewire`.

use std::collections::HashMap;
use std::path::Path;

use tower_lsp_server::ls_types::{Location, Position, Uri};

use crate::text::zero_width_location;

use super::pascal_to_kebab;

#[derive(Debug, Default, Clone)]
pub struct LivewireIndex {
    components: HashMap<String, Location>,
}

impl LivewireIndex {
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
        walk_base(&root.join("app").join("Livewire"), &mut components);
        walk_base(&root.join("app").join("Http").join("Livewire"), &mut components);
        Self { components }
    }
}

fn walk_base(base: &Path, out: &mut HashMap<String, Location>) {
    walk(base, base, out);
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
        let Some(name) = component_name_for(base, &path) else {
            continue;
        };
        let Some(uri) = Uri::from_file_path(&path) else {
            continue;
        };
        out.entry(name)
            .or_insert_with(|| zero_width_location(&uri, 0));
    }
}

/// Kebab-dotted component name for `path` (a `.php` file), relative to
/// `base` (`app/Livewire` or `app/Http/Livewire`) —
/// `Forms/Counter.php` -> `forms.counter`.
fn component_name_for(base: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let rel_str = rel.to_str()?.replace('\\', "/");
    let stem = rel_str.strip_suffix(".php")?;
    Some(
        stem.split('/')
            .map(pascal_to_kebab)
            .collect::<Vec<_>>()
            .join("."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_component(root: &Path, dir: &str, rel: &str, contents: &str) {
        let path = root.join(dir).join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn indexes_livewire3_convention() {
        let tmp = tempfile::tempdir().unwrap();
        write_component(
            tmp.path(),
            "app/Livewire",
            "Counter.php",
            "<?php class Counter {}\n",
        );
        write_component(
            tmp.path(),
            "app/Livewire",
            "Forms/PostForm.php",
            "<?php class PostForm {}\n",
        );
        let idx = LivewireIndex::load(tmp.path());
        assert!(idx.get("counter").is_some());
        assert!(idx.get("forms.post-form").is_some());
    }

    #[test]
    fn indexes_legacy_http_livewire_convention() {
        let tmp = tempfile::tempdir().unwrap();
        write_component(
            tmp.path(),
            "app/Http/Livewire",
            "Counter.php",
            "<?php class Counter {}\n",
        );
        let idx = LivewireIndex::load(tmp.path());
        assert!(idx.get("counter").is_some());
    }

    #[test]
    fn missing_dirs_yield_empty_index() {
        let tmp = tempfile::tempdir().unwrap();
        let idx = LivewireIndex::load(tmp.path());
        assert_eq!(idx.names().count(), 0);
    }
}
