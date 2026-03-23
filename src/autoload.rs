use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// PSR-4 namespace-prefix → base-directory mapping built from `composer.json`
/// and `vendor/composer/installed.json`.
///
/// Used to resolve a fully-qualified class name to a source file when the
/// class is not yet in the workspace index.
pub struct Psr4Map {
    /// Sorted longest-prefix-first so the most-specific prefix always wins.
    entries: Vec<(String, Vec<PathBuf>)>,
}

impl Psr4Map {
    pub fn empty() -> Self {
        Psr4Map { entries: vec![] }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Build a map from the project root. Reads:
    /// - `<root>/composer.json`           (project namespaces, incl. autoload-dev)
    /// - `<root>/vendor/composer/installed.json`  (all installed packages)
    pub fn load(root: &Path) -> Self {
        let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();

        // Project's own composer.json (both autoload and autoload-dev)
        add_from_composer_json(&root.join("composer.json"), root, &mut map);

        // Installed packages via vendor/composer/installed.json
        let installed_json = root.join("vendor").join("composer").join("installed.json");
        if let Ok(text) = std::fs::read_to_string(&installed_json) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                // Composer v2: {"packages": [...]}  |  Composer v1: [...]
                let packages = json
                    .get("packages")
                    .and_then(|v| v.as_array())
                    .or_else(|| json.as_array());

                if let Some(pkgs) = packages {
                    let vendor_composer = root.join("vendor").join("composer");
                    for pkg in pkgs {
                        let install_path = pkg
                            .get("install-path")
                            .and_then(|v| v.as_str())
                            .map(|p| vendor_composer.join(p));

                        if let Some(pkg_root) = install_path {
                            let pkg_root =
                                std::fs::canonicalize(&pkg_root).unwrap_or(pkg_root);
                            if let Some(autoload) = pkg.get("autoload") {
                                load_psr4_section(autoload, &pkg_root, &mut map);
                            }
                        }
                    }
                }
            }
        }

        // Longest-prefix-first so most-specific match wins
        let mut entries: Vec<_> = map.into_iter().collect();
        entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

        Psr4Map { entries }
    }

    /// Resolve a fully-qualified class name to an existing file on disk.
    ///
    /// `class_name` may or may not have a leading `\`.
    /// Returns `None` if no prefix matches or the resolved file doesn't exist.
    pub fn resolve(&self, class_name: &str) -> Option<PathBuf> {
        let class_name = class_name.trim_start_matches('\\');

        for (prefix, base_dirs) in &self.entries {
            // Strip the trailing `\` from the namespace prefix for comparison
            let ns = prefix.trim_end_matches('\\');
            if !class_name.starts_with(ns) {
                continue;
            }
            let remainder = class_name[ns.len()..].trim_start_matches('\\');
            let relative = remainder.replace('\\', "/") + ".php";

            for base in base_dirs {
                let candidate = base.join(&relative);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

fn add_from_composer_json(
    composer_json: &Path,
    root: &Path,
    map: &mut HashMap<String, Vec<PathBuf>>,
) {
    let Ok(text) = std::fs::read_to_string(composer_json) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    for section in ["autoload", "autoload-dev"] {
        if let Some(autoload) = json.get(section) {
            load_psr4_section(autoload, root, map);
        }
    }
}

fn load_psr4_section(
    autoload: &serde_json::Value,
    root: &Path,
    map: &mut HashMap<String, Vec<PathBuf>>,
) {
    let Some(psr4) = autoload.get("psr-4").and_then(|v| v.as_object()) else {
        return;
    };
    for (ns, paths) in psr4 {
        let dirs: Vec<PathBuf> = match paths {
            serde_json::Value::String(s) => vec![root.join(s)],
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| root.join(s))
                .collect(),
            _ => continue,
        };
        map.entry(ns.clone()).or_default().extend(dirs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn empty_map_resolves_nothing() {
        let m = Psr4Map::empty();
        assert!(m.is_empty());
        assert!(m.resolve("App\\Foo").is_none());
    }

    #[test]
    fn resolves_class_from_composer_json() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Create the target file
        write(&root.join("src/Services/Foo.php"), "<?php class Foo {}");

        // Create composer.json
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
        );

        let m = Psr4Map::load(root);
        assert!(!m.is_empty());

        let resolved = m.resolve("App\\Services\\Foo");
        assert!(resolved.is_some(), "should resolve App\\Services\\Foo");
        assert!(resolved.unwrap().ends_with("src/Services/Foo.php"));
    }

    #[test]
    fn returns_none_when_file_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
        );
        let m = Psr4Map::load(root);
        assert!(m.resolve("App\\Missing\\Class").is_none());
    }

    #[test]
    fn leading_backslash_is_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("src/Foo.php"), "<?php class Foo {}");
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
        );
        let m = Psr4Map::load(root);
        // \App\Foo and App\Foo should both work
        assert!(m.resolve("\\App\\Foo").is_some());
        assert!(m.resolve("App\\Foo").is_some());
    }

    #[test]
    fn longer_prefix_wins_over_shorter() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("src/Foo.php"), "<?php");
        write(&root.join("core/Foo.php"), "<?php");
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/", "App\\Core\\": "core/"}}}"#,
        );
        let m = Psr4Map::load(root);
        // App\Core\Foo should resolve to core/Foo.php, not src/Core/Foo.php
        let resolved = m.resolve("App\\Core\\Foo").unwrap();
        assert!(resolved.ends_with("core/Foo.php"), "got {:?}", resolved);
    }

    #[test]
    fn loads_empty_when_composer_json_absent() {
        let dir = tempfile::tempdir().unwrap();
        let m = Psr4Map::load(dir.path());
        assert!(m.is_empty());
    }

    #[test]
    fn autoload_dev_entries_are_included() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("tests/FooTest.php"), "<?php");
        write(
            &root.join("composer.json"),
            r#"{"autoload-dev": {"psr-4": {"Tests\\": "tests/"}}}"#,
        );
        let m = Psr4Map::load(root);
        assert!(m.resolve("Tests\\FooTest").is_some());
    }
}
