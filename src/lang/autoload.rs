use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// Clamp an unsupported PHP version string to the nearest supported version.
///
/// The version is parsed as `"major.minor"`. If it is below the minimum
/// supported version it is clamped to `PHP_7_4`; if it is above the maximum
/// it is clamped to `PHP_8_5`. Already-valid versions are returned unchanged.
pub fn clamp_php_version(v: &str) -> &'static str {
    if is_valid_php_version(v) {
        return SUPPORTED_PHP_VERSIONS
            .iter()
            .find(|&&s| s == v)
            .copied()
            .unwrap_or(PHP_8_5);
    }
    let (major, minor) = match v.split_once('.') {
        Some((maj, min)) => {
            // min may be "1.5" for patch versions like "8.1.5" — take only the first token.
            let min_part = min.split('.').next().unwrap_or(min);
            (
                maj.parse::<u32>().unwrap_or(0),
                min_part.parse::<u32>().unwrap_or(0),
            )
        }
        None => (v.parse::<u32>().unwrap_or(0), 0),
    };
    // SUPPORTED_PHP_VERSIONS is sorted ascending; pick the closest boundary.
    let (min_maj, min_min) = SUPPORTED_PHP_VERSIONS
        .first()
        .and_then(|s| s.split_once('.'))
        .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
        .unwrap_or((7, 4));
    let (max_maj, max_min) = SUPPORTED_PHP_VERSIONS
        .last()
        .and_then(|s| s.split_once('.'))
        .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
        .unwrap_or((8, 5));
    if (major, minor) < (min_maj, min_min) {
        SUPPORTED_PHP_VERSIONS.first().copied().unwrap_or(PHP_7_4)
    } else if (major, minor) > (max_maj, max_min) {
        SUPPORTED_PHP_VERSIONS.last().copied().unwrap_or(PHP_8_5)
    } else {
        // Between min and max but not in the list — pick the highest version ≤ input.
        SUPPORTED_PHP_VERSIONS
            .iter()
            .rfind(|&&s| {
                let (a, b) = s
                    .split_once('.')
                    .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
                    .unwrap_or((0, 0));
                (a, b) <= (major, minor)
            })
            .copied()
            .unwrap_or(PHP_7_4)
    }
}

/// PSR-4 namespace-prefix → base-directory mapping.
///
/// Wraps `mir_analyzer::Psr4Map` for FQN→path resolution and `ClassResolver`,
/// and maintains a lightweight project-only reverse index for the path→FQN
/// lookup needed by file rename/delete handlers.
pub struct Psr4Map {
    /// One resolved map per workspace root. Used for `resolve` and as the
    /// `ClassResolver` injected into the mir analyzer session.
    inners: Vec<Arc<mir_analyzer::Psr4Map>>,
    /// PSR-4 entries from the project's own `autoload`/`autoload-dev` sections
    /// only. Vendor packages are excluded: rename/delete only targets project
    /// files, so the reverse index never needs vendor entries.
    project_entries: Vec<(String, PathBuf)>,
    /// PSR-0 entries from all `autoload.psr-0` sections (project + vendor).
    /// Used as a fallback when PSR-4 resolution returns `None`.
    psr0_entries: Vec<(String, PathBuf)>,
}

impl mir_analyzer::ClassResolver for Psr4Map {
    fn resolve(&self, fqcn: &str) -> Option<PathBuf> {
        self.inners.iter().find_map(|m| m.resolve(fqcn))
    }
}

impl Psr4Map {
    pub fn empty() -> Self {
        Psr4Map {
            inners: vec![],
            project_entries: vec![],
            psr0_entries: vec![],
        }
    }

    /// Build a map from a workspace root. Delegates all resolution logic to
    /// `mir_analyzer::Psr4Map` (which handles PSR-4, PSR-0, classmap, vendor
    /// eager files, and `autoload_classmap.php`). Also extracts project PSR-4
    /// entries for the `file_to_fqn` reverse index.
    ///
    /// If `root` itself has no `composer.json`, walks up parent directories to
    /// find one (up to 8 levels). This handles workspaces rooted at a `src/`
    /// subdirectory where the project's `composer.json` lives one level up.
    pub fn load(root: &Path) -> Self {
        let composer_root = find_composer_root(root).unwrap_or_else(|| root.to_path_buf());
        let mut inners = Vec::new();
        if let Ok(map) = mir_analyzer::Psr4Map::from_composer(&composer_root) {
            inners.push(Arc::new(map));
        }
        let project_entries = read_project_psr4_entries(&composer_root);
        let psr0_entries = read_psr0_entries(&composer_root);
        Psr4Map {
            inners,
            project_entries,
            psr0_entries,
        }
    }

    /// Merge another map's entries (for multi-root workspaces).
    pub fn extend(&mut self, other: Psr4Map) {
        self.inners.extend(other.inners);
        self.project_entries.extend(other.project_entries);
        self.project_entries
            .sort_by_key(|e| std::cmp::Reverse(e.0.len()));
        self.psr0_entries.extend(other.psr0_entries);
        self.psr0_entries
            .sort_by_key(|e| std::cmp::Reverse(e.0.len()));
    }

    pub fn project_namespace_count(&self) -> usize {
        self.project_entries.len()
    }

    /// Reverse of `resolve`: given a file path, return the PSR-4 FQN, or
    /// `None` if the path isn't under a known project namespace prefix.
    pub fn file_to_fqn(&self, path: &Path) -> Option<String> {
        for (prefix, dir) in &self.project_entries {
            if let Ok(rel) = path.strip_prefix(dir) {
                let rel_str = rel.to_string_lossy();
                let without_ext = rel_str.strip_suffix(".php")?;
                let class_path = without_ext.replace([std::path::MAIN_SEPARATOR, '/'], "\\");
                return Some(format!("{}{}", prefix, class_path));
            }
        }
        None
    }

    /// Resolve a fully-qualified class name to an existing file on disk.
    pub fn resolve(&self, fqcn: &str) -> Option<PathBuf> {
        self.inners.iter().find_map(|m| m.resolve(fqcn))
    }

    /// Resolve a PSR-0 class name to an existing file on disk.
    ///
    /// PSR-0 maps `_` in class names to directory separators. For example,
    /// `Acme_Client` with prefix `Acme_` and base `vendor/acme/lib/src/`
    /// resolves to `vendor/acme/lib/src/Acme/Client.php`.
    pub fn psr0_resolve(&self, class_name: &str) -> Option<PathBuf> {
        let class_name = class_name.trim_start_matches('\\');
        // Sort by longest prefix first (already guaranteed by extend/load).
        for (prefix, base_dir) in &self.psr0_entries {
            if class_name.starts_with(prefix.as_str()) {
                // PSR-0: replace `\` and `_` with the path separator.
                let file_path = class_name.replace(['\\', '_'], "/");
                let candidate = base_dir.join(format!("{file_path}.php"));
                if candidate.exists() {
                    return Some(candidate);
                }
            }
        }
        None
    }
}

/// Walk up from `start` to find the nearest directory containing `composer.json`.
/// Returns `None` if not found within 8 levels. This handles workspaces rooted
/// at a `src/` subdirectory where `composer.json` lives in the project root above.
fn find_composer_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    for _ in 0..8 {
        if dir.join("composer.json").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
    None
}

/// Extract PSR-4 project entries from `composer.json` at `root`.
/// Only reads `autoload` and `autoload-dev` psr-4 sections; vendor packages
/// are omitted because `file_to_fqn` is only called on project files.
fn read_project_psr4_entries(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(text) = std::fs::read_to_string(root.join("composer.json")) else {
        return vec![];
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for section in ["autoload", "autoload-dev"] {
        let Some(psr4) = json
            .get(section)
            .and_then(|a| a.get("psr-4"))
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        for (ns, paths) in psr4 {
            let prefix = if ns.ends_with('\\') {
                ns.clone()
            } else {
                format!("{ns}\\")
            };
            match paths {
                serde_json::Value::String(s) => {
                    entries.push((prefix, root.join(s)));
                }
                serde_json::Value::Array(arr) => {
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            entries.push((prefix.clone(), root.join(s)));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    entries.sort_by_key(|e| std::cmp::Reverse(e.0.len()));
    entries
}

/// Read PSR-0 entries from `composer.json` and `vendor/composer/installed.json`
/// at `root`. Returns `(prefix, base_dir)` pairs sorted by descending prefix
/// length so the longest prefix wins when multiple entries could match.
fn read_psr0_entries(root: &Path) -> Vec<(String, PathBuf)> {
    let mut entries: Vec<(String, PathBuf)> = Vec::new();

    // Project-level composer.json
    if let Ok(text) = std::fs::read_to_string(root.join("composer.json"))
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
    {
        for section in ["autoload", "autoload-dev"] {
            if let Some(psr0) = json
                .get(section)
                .and_then(|a| a.get("psr-0"))
                .and_then(|v| v.as_object())
            {
                collect_psr0_entries(psr0, root, &mut entries);
            }
        }
    }

    // Vendor packages via installed.json
    let installed_path = root.join("vendor/composer/installed.json");
    if let Ok(text) = std::fs::read_to_string(&installed_path)
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(&text)
    {
        let packages = json
            .get("packages")
            .and_then(|v| v.as_array())
            .or_else(|| json.as_array())
            .into_iter()
            .flatten();
        for pkg in packages {
            let install_path = pkg
                .get("install-path")
                .and_then(|v| v.as_str())
                .map(|p| installed_path.parent().unwrap_or(root).join(p))
                .unwrap_or_else(|| root.join("vendor"));
            if let Some(psr0) = pkg
                .get("autoload")
                .and_then(|a| a.get("psr-0"))
                .and_then(|v| v.as_object())
            {
                collect_psr0_entries(psr0, &install_path, &mut entries);
            }
        }
    }

    entries.sort_by_key(|e| std::cmp::Reverse(e.0.len()));
    entries
}

fn collect_psr0_entries(
    map: &serde_json::Map<String, serde_json::Value>,
    base: &Path,
    entries: &mut Vec<(String, PathBuf)>,
) {
    for (prefix, paths) in map {
        match paths {
            serde_json::Value::String(s) => {
                entries.push((prefix.clone(), base.join(s)));
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    if let Some(s) = item.as_str() {
                        entries.push((prefix.clone(), base.join(s)));
                    }
                }
            }
            _ => {}
        }
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

/// Detect the PHP version by running `php --version`, memoized on disk.
///
/// Booting the interpreter costs ~100–300 ms and this sits on the
/// `initialize` critical path, so the result is cached keyed by the resolved
/// binary's path + mtime + size — a PHP upgrade or PATH change re-detects.
pub fn detect_php_binary_version() -> Option<String> {
    let cache_file = crate::index::cache::cache_base_dir()
        .map(|d| d.join("php-lsp").join("php-binary-version.json"));
    detect_php_binary_version_with_cache(cache_file.as_deref())
}

fn detect_php_binary_version_with_cache(cache_file: Option<&Path>) -> Option<String> {
    let key = php_binary_cache_key();
    if let (Some(cache_file), Some(key)) = (cache_file, key.as_ref())
        && let Ok(text) = std::fs::read_to_string(cache_file)
        && let Ok(cached) = serde_json::from_str::<serde_json::Value>(&text)
        && cached.get("key").and_then(|k| k.as_str()) == Some(key)
        && let Some(ver) = cached.get("version").and_then(|v| v.as_str())
    {
        return Some(ver.to_string());
    }

    let output = std::process::Command::new("php")
        .arg("--version")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // First line: "PHP X.Y.Z (cli) ..."
    let first_line = stdout.lines().next()?;
    let version_str = first_line.strip_prefix("PHP ")?.split_whitespace().next()?;
    let ver = extract_major_minor(version_str)?;

    if let (Some(cache_file), Some(key)) = (cache_file, key) {
        let entry = serde_json::json!({"key": key, "version": ver});
        if let Some(parent) = cache_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(cache_file, entry.to_string());
    }
    Some(ver)
}

/// Identity of the `php` binary `Command::new("php")` would spawn:
/// resolved path + mtime + size. `None` when it can't be resolved via PATH —
/// callers then skip the cache and always spawn.
fn php_binary_cache_key() -> Option<String> {
    let exe = if cfg!(windows) { "php.exe" } else { "php" };
    let path = std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(exe))
            .find(|c| c.is_file())
    })?;
    let meta = std::fs::metadata(&path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(format!("{}|{mtime}|{}", path.display(), meta.len()))
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
        assert!(m.resolve("\\App\\Foo").unwrap().ends_with("src/Foo.php"));
        assert!(m.resolve("App\\Foo").unwrap().ends_with("src/Foo.php"));
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
        assert!(m.resolve("App\\Foo").is_none());
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
        assert!(
            m.resolve("Tests\\FooTest")
                .unwrap()
                .ends_with("tests/FooTest.php")
        );
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

    #[test]
    fn php_binary_version_cache_round_trips_and_invalidates_on_key_change() {
        // Only meaningful where a php binary resolves via PATH.
        let Some(key) = php_binary_cache_key() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let cache_file = dir.path().join("php-binary-version.json");

        // A fresh cache entry for the current binary is served without spawning.
        std::fs::write(
            &cache_file,
            serde_json::json!({"key": key, "version": "7.0"}).to_string(),
        )
        .unwrap();
        assert_eq!(
            detect_php_binary_version_with_cache(Some(&cache_file)),
            Some("7.0".to_string()),
            "matching cache entry must be served as-is"
        );

        // A stale key (different binary identity) forces re-detection, which
        // overwrites the entry with the real version.
        std::fs::write(
            &cache_file,
            serde_json::json!({"key": "other|0|0", "version": "7.0"}).to_string(),
        )
        .unwrap();
        let detected = detect_php_binary_version_with_cache(Some(&cache_file));
        assert_ne!(
            detected,
            Some("7.0".to_string()),
            "stale cache key must not be served"
        );
        if let Some(ref v) = detected {
            let rewritten: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&cache_file).unwrap()).unwrap();
            assert_eq!(rewritten["version"].as_str(), Some(v.as_str()));
            assert_eq!(rewritten["key"].as_str(), Some(key.as_str()));
        }
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

    // --- clamp_php_version ---

    #[test]
    fn clamp_valid_version_unchanged() {
        for &v in SUPPORTED_PHP_VERSIONS {
            assert_eq!(clamp_php_version(v), v);
        }
    }

    #[test]
    fn clamp_old_version_to_minimum() {
        assert_eq!(clamp_php_version("5.6"), PHP_7_4);
        assert_eq!(clamp_php_version("7.0"), PHP_7_4);
        assert_eq!(clamp_php_version("7.3"), PHP_7_4);
    }

    #[test]
    fn clamp_future_version_to_maximum() {
        assert_eq!(clamp_php_version("9.0"), PHP_8_5);
        assert_eq!(clamp_php_version("10.1"), PHP_8_5);
    }

    #[test]
    fn clamp_between_versions_picks_highest_below() {
        // 8.1.5 is not in the list but should clamp to 8.1
        assert_eq!(clamp_php_version("8.1.5"), PHP_8_1);
    }
}
