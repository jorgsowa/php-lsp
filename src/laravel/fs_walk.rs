//! Recursive `.php` file discovery, shared by `ConfigIndex`, `TranslationIndex`,
//! and `RouteIndex`'s directory scans — all three index a whole subtree
//! (`config/`, `lang/{locale}/`, `routes/`) rather than just its direct
//! children.

use std::path::{Path, PathBuf};

/// Every `.php` file under `dir`, recursing into subdirectories, sorted by
/// path for deterministic iteration order.
pub(super) fn php_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "php") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nested_php_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        std::fs::write(tmp.path().join("top.php"), "").unwrap();
        std::fs::write(tmp.path().join("a/mid.php"), "").unwrap();
        std::fs::write(tmp.path().join("a/b/deep.php"), "").unwrap();
        std::fs::write(tmp.path().join("a/b/ignored.txt"), "").unwrap();
        let files = php_files_recursive(tmp.path());
        let names: Vec<&str> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(names, vec!["deep.php", "mid.php", "top.php"]);
    }

    #[test]
    fn missing_dir_yields_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(php_files_recursive(&tmp.path().join("nope")).is_empty());
    }
}
