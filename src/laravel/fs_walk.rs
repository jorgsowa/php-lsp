//! Recursive file discovery, shared by the Laravel indexes that cover a whole
//! subtree (`config/`, `lang/{locale}/`, `routes/`, views, components, assets).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Every `.php` file under `dir`, recursing into subdirectories, sorted by
/// path for deterministic iteration order.
pub(super) fn php_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = files_recursive(dir);
    out.retain(|p| p.extension().is_some_and(|e| e == "php"));
    out
}

/// Every file under `dir`, sorted, following symlinks; a directory reached
/// through several links is walked once.
pub(super) fn files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect(dir, &mut HashSet::new(), &mut out);
    out.sort();
    out
}

fn collect(dir: &Path, visited: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
    if !dir.canonicalize().is_ok_and(|c| visited.insert(c)) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, visited, out);
        } else if path.is_file() {
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

    #[cfg(unix)]
    #[test]
    fn follows_symlinked_dirs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let shared = tmp.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("a.php"), "").unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(&shared, root.join("linked")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        std::os::unix::fs::symlink(shared.join("a.php"), root.join("b.php")).unwrap();
        let files = php_files_recursive(&root);
        let rel: Vec<&Path> = files
            .iter()
            .map(|p| p.strip_prefix(&root).unwrap())
            .collect();
        assert_eq!(rel, [Path::new("b.php"), Path::new("linked/a.php")]);
    }

    #[test]
    fn missing_dir_yields_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(php_files_recursive(&tmp.path().join("nope")).is_empty());
    }
}
