use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Supported PHP version strings.
pub const PHP_7_4: &str = "7.4";
pub const PHP_8_0: &str = "8.0";
pub const PHP_8_1: &str = "8.1";
pub const PHP_8_2: &str = "8.2";
pub const PHP_8_3: &str = "8.3";
pub const PHP_8_4: &str = "8.4";
pub const PHP_8_5: &str = "8.5";

pub const SUPPORTED_PHP_VERSIONS: &[&str] = &[
    PHP_7_4, PHP_8_0, PHP_8_1, PHP_8_2, PHP_8_3, PHP_8_4, PHP_8_5,
];

pub fn is_valid_php_version(v: &str) -> bool {
    SUPPORTED_PHP_VERSIONS.contains(&v)
}

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

    /// Build a map from the project root. Reads:
    /// - `<root>/composer.json`           (project namespaces, incl. autoload-dev)
    /// - `<root>/vendor/composer/installed.json`  (all installed packages)
    pub fn load(root: &Path) -> Self {
        let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();

        // Project's own composer.json (both autoload and autoload-dev)
        add_from_composer_json(&root.join("composer.json"), root, &mut map);

        // Installed packages via vendor/composer/installed.json
        let installed_json = root.join("vendor").join("composer").join("installed.json");
        if let Ok(text) = std::fs::read_to_string(&installed_json)
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
        {
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
                        let pkg_root = std::fs::canonicalize(&pkg_root).unwrap_or(pkg_root);
                        if let Some(autoload) = pkg.get("autoload") {
                            load_psr4_section(autoload, &pkg_root, &mut map);
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

    /// Merge another map's entries into this one, maintaining longest-prefix-first order.
    pub fn extend(&mut self, other: Psr4Map) {
        self.entries.extend(other.entries);
        self.entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    }

    /// Reverse of `resolve`: given a file path, return the PSR-4 fully-qualified
    /// class name that maps to it, or `None` if the path doesn't fall under any
    /// known namespace prefix.
    pub fn file_to_fqn(&self, path: &Path) -> Option<String> {
        for (prefix, base_dirs) in &self.entries {
            for base in base_dirs {
                if let Ok(rel) = path.strip_prefix(base) {
                    let rel_str = rel.to_string_lossy();
                    let without_ext = rel_str.strip_suffix(".php")?;
                    // Normalise path separators to backslashes (PSR-4 uses `\`)
                    let class_path = without_ext.replace([std::path::MAIN_SEPARATOR, '/'], "\\");
                    return Some(format!("{}{}", prefix, class_path));
                }
            }
        }
        None
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
            let after = &class_name[ns.len()..];
            // Require `\` or end-of-string after the prefix so that "App" does
            // not match "Application\Foo".
            if !after.is_empty() && !after.starts_with('\\') {
                continue;
            }
            let remainder = after.trim_start_matches('\\');
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

/// Detect the PHP version from a project's `composer.json`.
///
/// Checks in order:
/// 1. `config.platform.php` — explicit platform override (e.g. `"8.1.0"`)
/// 2. `require.php` — version constraint lower bound (e.g. `"^8.1"` → `"8.1"`)
pub fn detect_php_version_from_composer(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("composer.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;

    // config.platform.php is the authoritative platform version override.
    if let Some(platform_php) = json
        .pointer("/config/platform/php")
        .and_then(|v| v.as_str())
        && let Some(ver) = extract_major_minor(platform_php)
    {
        return Some(ver);
    }

    // require.php is the minimum version constraint.
    if let Some(constraint) = json.pointer("/require/php").and_then(|v| v.as_str())
        && let Some(ver) = parse_php_version_constraint(constraint)
    {
        return Some(ver);
    }

    None
}

/// Detect the PHP version by running `php --version`.
pub fn detect_php_binary_version() -> Option<String> {
    let output = std::process::Command::new("php")
        .arg("--version")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // First line: "PHP X.Y.Z (cli) ..."
    let first_line = stdout.lines().next()?;
    let version_str = first_line.strip_prefix("PHP ")?.split_whitespace().next()?;
    extract_major_minor(version_str)
}

/// Extract `"X.Y"` from a full version string like `"8.1.27"` or `"8.2"`.
fn extract_major_minor(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major = parts.next()?.trim();
    let minor = parts.next()?.trim();
    major.parse::<u32>().ok()?;
    minor.parse::<u32>().ok()?;
    Some(format!("{}.{}", major, minor))
}

/// Extract a `"X.Y"` lower bound from a Composer version constraint like
/// `"^8.1"`, `">=8.0"`, `"~8.2"`, `"7.4.*"`, or `">=8.0 <9.0"`.
fn parse_php_version_constraint(constraint: &str) -> Option<String> {
    // Take the first OR-clause: ">=7.4 || ^8.0" → ">=7.4"
    let clause = constraint.split("||").next().unwrap_or(constraint).trim();
    // Strip leading comparison/range operators
    let stripped = clause.trim_start_matches(['^', '~', '>', '<', '=', ' ']);
    // Take the first whitespace-delimited token: "8.0 <9.0" → "8.0"
    let token = stripped.split_whitespace().next().unwrap_or(stripped);
    // Split on '.' to get major and minor, stripping trailing wildcards
    let mut parts = token.split('.');
    let major = parts.next()?;
    let minor_raw = parts.next().unwrap_or("0");
    let minor = minor_raw.trim_end_matches('*');
    let minor = if minor.is_empty() { "0" } else { minor };
    major.parse::<u32>().ok()?;
    minor.parse::<u32>().ok()?;
    Some(format!("{}.{}", major, minor))
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
        assert!(m.entries.is_empty());
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
        assert!(!m.entries.is_empty());

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
        assert!(m.entries.is_empty());
    }

    #[test]
    fn psr4_prefix_does_not_match_longer_namespace() {
        // "App\" prefix must not resolve "Application\Foo" (substring false positive).
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // Create the file that would be resolved if the bug were present.
        write(&root.join("src/lication/Foo.php"), "<?php");
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
        );
        let m = Psr4Map::load(root);
        // "Application\Foo" must NOT resolve via the "App\" prefix.
        assert!(
            m.resolve("Application\\Foo").is_none(),
            "App\\ prefix must not match Application\\Foo"
        );
    }

    #[test]
    fn psr4_exact_prefix_still_resolves() {
        // Confirm that "App\Foo" still resolves correctly after the boundary fix.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("src/Foo.php"), "<?php");
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
        );
        let m = Psr4Map::load(root);
        assert!(m.resolve("App\\Foo").is_some(), "App\\Foo must resolve");
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

    // --- PHP version detection ---

    #[test]
    fn detect_version_from_platform_config() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"config": {"platform": {"php": "8.1.27"}}}"#,
        );
        assert_eq!(
            detect_php_version_from_composer(dir.path()),
            Some(PHP_8_1.to_string())
        );
    }

    #[test]
    fn detect_version_from_require_caret() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.2"}}"#,
        );
        assert_eq!(
            detect_php_version_from_composer(dir.path()),
            Some(PHP_8_2.to_string())
        );
    }

    #[test]
    fn detect_version_from_require_gte() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": ">=8.0"}}"#,
        );
        assert_eq!(
            detect_php_version_from_composer(dir.path()),
            Some(PHP_8_0.to_string())
        );
    }

    #[test]
    fn detect_version_from_require_range() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": ">=8.1 <9.0"}}"#,
        );
        assert_eq!(
            detect_php_version_from_composer(dir.path()),
            Some(PHP_8_1.to_string())
        );
    }

    #[test]
    fn detect_version_from_require_wildcard() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "7.4.*"}}"#,
        );
        assert_eq!(
            detect_php_version_from_composer(dir.path()),
            Some(PHP_7_4.to_string())
        );
    }

    #[test]
    fn platform_config_takes_priority_over_require() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"config": {"platform": {"php": "8.0.0"}}, "require": {"php": "^8.2"}}"#,
        );
        assert_eq!(
            detect_php_version_from_composer(dir.path()),
            Some(PHP_8_0.to_string())
        );
    }

    #[test]
    fn detect_version_returns_none_when_no_composer_json() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_php_version_from_composer(dir.path()).is_none());
    }

    #[test]
    fn detect_version_returns_none_when_no_php_entry() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"some/package": "^1.0"}}"#,
        );
        assert!(detect_php_version_from_composer(dir.path()).is_none());
    }
}
