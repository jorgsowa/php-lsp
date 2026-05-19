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
#[derive(Clone, Default)]
pub struct Psr4Map {
    /// Sorted longest-prefix-first so the most-specific prefix always wins.
    entries: Vec<(String, Vec<PathBuf>)>,
    /// When true, try a lowercase relative path if the exact PSR-4 path is missing.
    case_insensitive: bool,
    /// Lowercase FQCN → file path (from `autoload.classmap` in `.php-lsp.json`).
    classmap: HashMap<String, PathBuf>,
}

impl mir_analyzer::ClassResolver for Psr4Map {
    fn resolve(&self, fqcn: &str) -> Option<PathBuf> {
        Psr4Map::resolve(self, fqcn)
    }
}

impl Psr4Map {
    pub fn empty() -> Self {
        Psr4Map {
            entries: vec![],
            case_insensitive: false,
            classmap: HashMap::new(),
        }
    }

    fn sort_entries(&mut self) {
        self.entries.sort_by_key(|e| std::cmp::Reverse(e.0.len()));
    }

    /// Merge extra PSR-4 namespaces, classmap files, and flags from `.php-lsp.json`
    /// `autoload` or `initializationOptions.autoload` (Composer-compatible shape).
    pub fn merge_project_autoload(&mut self, root: &Path, autoload: &serde_json::Value) {
        if let Some(case_insensitive) = autoload.get("caseInsensitive").and_then(|v| v.as_bool())
        {
            self.case_insensitive = case_insensitive;
        }

        let mut extra: HashMap<String, Vec<PathBuf>> = HashMap::new();
        if let Some(psr4) = autoload.get("psr-4") {
            load_psr4_section(
                &serde_json::json!({ "psr-4": psr4 }),
                root,
                &mut extra,
            );
        }
        for (ns, dirs) in extra {
            let entry = self
                .entries
                .iter_mut()
                .find(|(prefix, _)| prefix == &ns);
            if let Some((_, existing)) = entry {
                for dir in dirs {
                    if !existing.contains(&dir) {
                        existing.push(dir);
                    }
                }
            } else {
                self.entries.push((ns, dirs));
            }
        }
        self.sort_entries();

        if let Some(files) = autoload.get("classmap").and_then(|v| v.as_array()) {
            for file in files {
                if let Some(rel) = file.as_str() {
                    self.load_classmap_json(&root.join(rel));
                }
            }
        }
    }

    fn load_classmap_json(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return;
        };
        let Some(obj) = json.as_object() else {
            return;
        };
        let base = path.parent().unwrap_or(Path::new("."));
        for (class, file_val) in obj {
            let Some(rel) = file_val.as_str() else {
                continue;
            };
            let file_path = base.join(rel);
            if file_path.exists() {
                self.classmap
                    .insert(class.trim_start_matches('\\').to_lowercase(), file_path);
            }
        }
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
        entries.sort_by_key(|e| std::cmp::Reverse(e.0.len()));

        Psr4Map {
            entries,
            case_insensitive: false,
            classmap: HashMap::new(),
        }
    }

    /// Merge another map's entries into this one, maintaining longest-prefix-first order.
    pub fn extend(&mut self, other: Psr4Map) {
        self.entries.extend(other.entries);
        self.sort_entries();
        self.classmap.extend(other.classmap);
        self.case_insensitive = self.case_insensitive || other.case_insensitive;
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

        if let Some(path) = self.classmap.get(&class_name.to_lowercase()) {
            if path.exists() {
                return Some(path.clone());
            }
        }

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
                if self.case_insensitive {
                    let relative_lower = remainder.to_lowercase().replace('\\', "/") + ".php";
                    let candidate_lower = base.join(&relative_lower);
                    if candidate_lower.exists() {
                        return Some(candidate_lower);
                    }
                }
            }
        }
        None
    }
}

/// Build a merged PSR-4 map for all workspace roots, applying project autoload overrides.
pub fn build_psr4_for_roots(roots: &[PathBuf], project_autoload: Option<&serde_json::Value>) -> Psr4Map {
    let mut merged = Psr4Map::empty();
    for root in roots {
        let mut map = Psr4Map::load(root);
        if let Some(autoload) = project_autoload {
            map.merge_project_autoload(root, autoload);
        }
        merged.extend(map);
    }
    merged
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

/// Detect PHP version from `config.platform.php` in `composer.json`.
///
/// This is an explicit developer override that tells Composer to treat a
/// specific PHP version as the runtime (commonly used to lock CI). It is
/// the most authoritative composer-based source.
pub fn detect_php_platform_version_from_composer(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("composer.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let platform_php = json.pointer("/config/platform/php")?.as_str()?;
    extract_major_minor(platform_php)
}

/// Detect PHP version from `require.php` in `composer.json`.
///
/// This is a compatibility range, not the exact runtime version. Use as a
/// last resort after `detect_php_platform_version_from_composer` and
/// `detect_php_binary_version`.
pub fn detect_php_require_version_from_composer(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("composer.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let constraint = json.pointer("/require/php")?.as_str()?;
    parse_php_version_constraint(constraint)
}

/// Load `.php-lsp.json` from the first workspace root that contains one.
///
/// Returns `None` if no root has the file or the file contains invalid JSON
/// (the caller should log a warning in that case and proceed with defaults).
pub fn load_project_config_json(roots: &[PathBuf]) -> Option<serde_json::Value> {
    for root in roots {
        let path = root.join(".php-lsp.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        return match serde_json::from_str(&text) {
            Ok(v) => Some(v),
            Err(_) => {
                // Signal parse failure by returning an explicit null so the
                // caller can distinguish "file not found" from "file corrupt".
                Some(serde_json::Value::Null)
            }
        };
    }
    None
}

/// Resolve the PHP version to use, in priority order:
///
/// 1. `explicit` — set by the client via `initializationOptions` or
///    `workspace/configuration` (highest priority).
/// 2. `phpVersion` in `.php-lsp.json` at the workspace root.
/// 3. `config.platform.php` in `composer.json` — explicit project-level override.
/// 4. `php --version` — actual runtime on the machine (or inside the container
///    when the LSP server runs there).
/// 5. `require.php` in `composer.json` — compatibility range, last resort.
/// 6. `PHP_8_5` — server default.
///
/// Returns `(version, source)` so the caller can log where the version came from.
pub fn resolve_php_version_from_roots(
    roots: &[PathBuf],
    explicit: Option<&str>,
) -> (String, &'static str) {
    if let Some(ver) = explicit {
        return (ver.to_string(), "set by editor");
    }
    // .php-lsp.json phpVersion (valid versions only; invalid ones are ignored here,
    // the caller logs a warning via load_project_config_json).
    if let Some(serde_json::Value::Object(obj)) = load_project_config_json(roots)
        && let Some(ver) = obj.get("phpVersion").and_then(|v| v.as_str())
        && is_valid_php_version(ver)
    {
        return (ver.to_string(), ".php-lsp.json");
    }
    if let Some(ver) = roots
        .iter()
        .find_map(|r| detect_php_platform_version_from_composer(r))
    {
        return (ver, "composer.json config.platform.php");
    }
    if let Some(ver) = detect_php_binary_version() {
        return (ver, "php binary");
    }
    if let Some(ver) = roots
        .iter()
        .find_map(|r| detect_php_require_version_from_composer(r))
    {
        return (ver, "composer.json require");
    }
    (PHP_8_5.to_string(), "default")
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

/// Extract the highest `"X.Y"` lower bound from a Composer version constraint
/// like `"^8.1"`, `">=8.0"`, `"~8.2"`, `"7.4.*"`, `">=8.0 <9.0"`, or
/// `"^7.4 || ^8.1"`.
///
/// For OR-constraints we take the **maximum** lower bound: a project that
/// declares `"^7.4 || ^8.1"` is most likely running on 8.1 locally, so using
/// the highest version gives the best LSP experience.
fn parse_php_version_constraint(constraint: &str) -> Option<String> {
    constraint
        .split("||")
        .filter_map(|clause| {
            // Strip leading comparison/range operators from the clause.
            let stripped = clause
                .trim()
                .trim_start_matches(['^', '~', '>', '<', '=', ' ']);
            // Take the first whitespace-delimited token: "8.0 <9.0" → "8.0".
            // TODO: for single-range constraints like ">=7.4 <9.0" this returns the
            // lower bound (7.4) rather than the actual runtime version. There is no
            // reliable way to infer the runtime from a range alone; the php binary
            // (detect_php_binary_version) is a better signal for that case.
            let token = stripped.split_whitespace().next().unwrap_or(stripped);
            // Split on '.' to get major and minor, stripping trailing wildcards.
            let mut parts = token.split('.');
            let major = parts.next()?;
            let minor_raw = parts.next().unwrap_or("0");
            let minor = minor_raw.trim_end_matches('*');
            let minor = if minor.is_empty() { "0" } else { minor };
            let maj: u32 = major.parse().ok()?;
            let min: u32 = minor.parse().ok()?;
            Some((maj, min, format!("{}.{}", major, minor)))
        })
        .max_by_key(|&(maj, min, _)| (maj, min))
        .map(|(_, _, ver)| ver)
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
            detect_php_platform_version_from_composer(dir.path()),
            Some(PHP_8_1.to_string())
        );
    }

    #[test]
    fn detect_platform_version_returns_none_when_no_platform_config() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.2"}}"#,
        );
        assert!(detect_php_platform_version_from_composer(dir.path()).is_none());
    }

    #[test]
    fn detect_version_from_require_caret() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.2"}}"#,
        );
        assert_eq!(
            detect_php_require_version_from_composer(dir.path()),
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
            detect_php_require_version_from_composer(dir.path()),
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
            detect_php_require_version_from_composer(dir.path()),
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
            detect_php_require_version_from_composer(dir.path()),
            Some(PHP_7_4.to_string())
        );
    }

    #[test]
    fn detect_version_returns_none_when_no_composer_json() {
        let dir = tempfile::tempdir().unwrap();
        assert!(detect_php_platform_version_from_composer(dir.path()).is_none());
        assert!(detect_php_require_version_from_composer(dir.path()).is_none());
    }

    #[test]
    fn detect_version_returns_none_when_no_php_entry() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"some/package": "^1.0"}}"#,
        );
        assert!(detect_php_require_version_from_composer(dir.path()).is_none());
    }

    #[test]
    fn detect_version_or_constraint_picks_highest() {
        // "^7.4 || ^8.1" — should return 8.1, not 7.4
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "^7.4 || ^8.1"}}"#,
        );
        assert_eq!(
            detect_php_require_version_from_composer(dir.path()),
            Some(PHP_8_1.to_string())
        );
    }

    #[test]
    fn detect_version_or_constraint_three_clauses() {
        // "^7.4 || ^8.0 || ^8.2" — should return 8.2
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "^7.4 || ^8.0 || ^8.2"}}"#,
        );
        assert_eq!(
            detect_php_require_version_from_composer(dir.path()),
            Some(PHP_8_2.to_string())
        );
    }

    #[test]
    fn detect_version_or_constraint_unsorted() {
        // Clauses in non-ascending order — should still return the maximum
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.0 || ^7.4 || ^8.1"}}"#,
        );
        assert_eq!(
            detect_php_require_version_from_composer(dir.path()),
            Some(PHP_8_1.to_string())
        );
    }

    // --- resolve_php_version_from_roots ---

    #[test]
    fn resolve_explicit_overrides_composer() {
        // Explicit version wins even when composer.json has a different platform version.
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"config": {"platform": {"php": "8.0.0"}}}"#,
        );
        let (ver, source) =
            resolve_php_version_from_roots(&[dir.path().to_path_buf()], Some("8.2"));
        assert_eq!(ver, "8.2");
        assert_eq!(source, "set by editor");
    }

    #[test]
    fn resolve_platform_beats_require() {
        // config.platform.php takes priority over require.php in the same composer.json.
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"config": {"platform": {"php": "8.0.0"}}, "require": {"php": "^8.2"}}"#,
        );
        let (ver, source) = resolve_php_version_from_roots(&[dir.path().to_path_buf()], None);
        assert_eq!(ver, PHP_8_0);
        assert_eq!(source, "composer.json config.platform.php");
    }

    #[test]
    fn resolve_require_used_as_last_resort() {
        // require.php is used when there is no platform config and the php binary
        // is absent. We simulate "no binary" by having a roots list that provides
        // a require constraint and asserting the source is "composer.json require"
        // OR "php binary" (if PHP happens to be installed in CI).
        //
        // We can only assert that the version is at least the require lower bound
        // since we cannot prevent the binary from being found.
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "^8.3"}}"#,
        );
        let (ver, source) = resolve_php_version_from_roots(&[dir.path().to_path_buf()], None);
        // If the binary was found its version may differ; what we can guarantee is
        // that the source is one of the expected values and the version parses.
        assert!(
            source == "php binary" || source == "composer.json require" || source == "default",
            "unexpected source: {source}"
        );
        assert!(ver.contains('.'), "version should be X.Y format, got {ver}");
    }

    #[test]
    fn resolve_tilde_constraint() {
        // "~8.1" means >=8.1 <9.0 — should detect 8.1.
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": "~8.1"}}"#,
        );
        assert_eq!(
            detect_php_require_version_from_composer(dir.path()),
            Some(PHP_8_1.to_string())
        );
    }

    #[test]
    fn resolve_default_when_no_composer_json_and_no_roots() {
        // With no roots at all and no binary we fall back to the default.
        // Since the binary may be present, accept either "php binary" or "default".
        let (ver, source) = resolve_php_version_from_roots(&[], None);
        assert!(
            source == "php binary" || source == "default",
            "unexpected source: {source}"
        );
        assert!(ver.contains('.'), "version should be X.Y format, got {ver}");
    }

    // --- parse_php_version_constraint edge cases ---

    #[test]
    fn constraint_empty_string_returns_none() {
        assert!(parse_php_version_constraint("").is_none());
    }

    #[test]
    fn constraint_wildcard_returns_none() {
        // "*" means any version — we can't pin to a specific one.
        assert!(parse_php_version_constraint("*").is_none());
    }

    #[test]
    fn constraint_major_only_without_minor() {
        // ">=8" has no minor component — treated as "8.0".
        assert_eq!(parse_php_version_constraint(">=8"), Some("8.0".to_string()));
    }

    // --- extract_major_minor edge cases ---

    #[test]
    fn platform_version_major_only_returns_none() {
        // "8" in config.platform.php has no minor — should not parse.
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"config": {"platform": {"php": "8"}}}"#,
        );
        assert!(detect_php_platform_version_from_composer(dir.path()).is_none());
    }

    // --- unsupported version ---

    #[test]
    fn resolve_unsupported_old_version_is_returned_from_require() {
        // ">=5.6" parses to "5.6" — not in SUPPORTED_PHP_VERSIONS.
        // resolve_php_version_from_roots still returns it; the caller is
        // responsible for emitting a warning (tested at the backend level).
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"require": {"php": ">=5.6"}}"#,
        );
        assert_eq!(
            detect_php_require_version_from_composer(dir.path()),
            Some("5.6".to_string())
        );
        assert!(!is_valid_php_version("5.6"));
    }

    // --- .php-lsp.json ---

    #[test]
    fn project_config_beats_composer_platform() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("composer.json"),
            r#"{"config": {"platform": {"php": "8.0.0"}}}"#,
        );
        write(
            &dir.path().join(".php-lsp.json"),
            r#"{"phpVersion": "8.3"}"#,
        );
        let (ver, source) = resolve_php_version_from_roots(&[dir.path().to_path_buf()], None);
        assert_eq!(ver, PHP_8_3);
        assert_eq!(source, ".php-lsp.json");
    }

    #[test]
    fn editor_explicit_beats_project_config() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".php-lsp.json"),
            r#"{"phpVersion": "8.1"}"#,
        );
        let (ver, source) =
            resolve_php_version_from_roots(&[dir.path().to_path_buf()], Some("8.4"));
        assert_eq!(ver, PHP_8_4);
        assert_eq!(source, "set by editor");
    }

    #[test]
    fn project_config_invalid_php_version_is_ignored() {
        // An unrecognised phpVersion in .php-lsp.json should be skipped; the
        // cascade falls through to composer / binary / default.
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join(".php-lsp.json"),
            r#"{"phpVersion": "5.3"}"#,
        );
        let (ver, source) = resolve_php_version_from_roots(&[dir.path().to_path_buf()], None);
        // Falls through to binary or default; the version is not "5.3".
        assert_ne!(ver, "5.3");
        assert_ne!(source, ".php-lsp.json");
    }

    #[test]
    fn load_project_config_json_returns_null_for_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(".php-lsp.json"), "not json {{{");
        let result = load_project_config_json(&[dir.path().to_path_buf()]);
        assert!(matches!(result, Some(serde_json::Value::Null)));
    }

    #[test]
    fn load_project_config_json_returns_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_project_config_json(&[dir.path().to_path_buf()]);
        assert!(result.is_none());
    }

    #[test]
    fn project_autoload_adds_extra_psr4_base_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "classes/"}}}"#,
        );
        write(&root.join("src/Extra/Foo.php"), "<?php namespace App\\Extra; class Foo {}");
        write(
            &root.join(".php-lsp.json"),
            r#"{"autoload": {"psr-4": {"App\\": ["src/"]}}}"#,
        );
        let map = build_psr4_for_roots(
            &[root.to_path_buf()],
            Some(&serde_json::json!({
                "psr-4": { "App\\": ["src/"] }
            })),
        );
        assert!(map.resolve("App\\Extra\\Foo").is_some());
    }

    #[test]
    fn case_insensitive_psr4_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("classes/logger/log_factory.php"), "<?php namespace WL\\Logger; class Log_factory {}");
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"WL\\": "classes/"}}}"#,
        );
        let mut map = Psr4Map::load(root);
        map.case_insensitive = true;
        assert!(map.resolve("WL\\Logger\\Log_factory").is_some());
    }

    #[test]
    fn classmap_json_resolves_lowercase_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("legacy/foo.class.php"), "<?php class foo {}");
        write(
            &root.join("classmap.json"),
            r#"{"foo": "legacy/foo.class.php"}"#,
        );
        let mut map = Psr4Map::empty();
        map.merge_project_autoload(
            root,
            &serde_json::json!({ "classmap": ["classmap.json"] }),
        );
        assert!(map.resolve("foo").is_some());
    }
}
