//! Workspace indexing: the compact per-file `FileIndex`, the background
//! workspace scan that builds it for every file, and the on-disk index cache.

pub mod file_index;

pub mod cache;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod workspace_scan;

pub(crate) fn matches_any(rel_path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| {
        let p = pat.trim_end_matches('*').trim_end_matches('/');
        rel_path.split('/').any(|c| c == p)
            || rel_path.starts_with(&format!("{p}/"))
            || rel_path.contains(&format!("/{p}/"))
            || rel_path
                .split('/')
                .any(|c| c.ends_with(".php") && c.strip_suffix(".php").unwrap_or(c) == p)
    })
}
