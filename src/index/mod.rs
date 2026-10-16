//! Workspace indexing: the compact per-file `FileIndex`, the background
//! workspace scan that builds it for every file, and the on-disk index cache.

pub mod file_index;

pub mod cache;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod workspace_scan;
