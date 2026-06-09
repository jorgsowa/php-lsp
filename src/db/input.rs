//! Salsa inputs.

use std::sync::Arc;

use mir_analyzer::PhpVersion;

use crate::file_index::FileIndex;

/// Immortal per-file text input. One per unique URI ever seen.
/// Holds the raw source text and the optional K2 on-disk cache seed.
///
/// `cached_index` (Phase K2): when `Some`, holds a pre-computed `FileIndex`
/// loaded from the on-disk cache. `file_index` checks this field first and
/// returns the cached index instead of parsing + extracting. Cleared back to
/// `None` on any text edit — see `DocumentStore::mirror_text` — so a stale
/// cached index cannot mask a real change. Seeded by workspace scan via
/// `DocumentStore::seed_cached_index` before the first `file_index` call for
/// that file.
#[salsa::input]
pub struct FileText {
    pub text: Arc<str>,
    pub cached_index: Option<Arc<FileIndex>>,
}

/// Tracked struct representing one active workspace file.
/// Produced by `workspace_files`; salsa GC's it (and all downstream memos)
/// when the file is removed from the workspace.
/// Identity fields: uri + text_input (both stable per unique URI).
#[salsa::tracked]
pub struct SourceFile<'db> {
    pub uri: Arc<str>,
    pub text_input: FileText,
}

/// Workspace-level input: the set of active (non-deleted) files.
/// Each entry is (uri_arc, FileText_handle) — sorted by uri for stable ordering.
/// Only changed when files are added or removed, not on text edits.
///
/// Uses `durability = HIGH` conceptually — the file list changes rarely
/// (workspace scan, deletions), not on every edit. Salsa's default durability
/// is LOW; backend can opt into HIGH via `set_files` if churn becomes an issue.
#[salsa::input]
pub struct Workspace {
    pub files: Arc<[(Arc<str>, FileText)]>,
    /// Target PHP version for analysis. Changing this invalidates all
    /// `semantic_issues` queries so diagnostics are re-evaluated against the
    /// new version's rules.
    pub php_version: PhpVersion,
}

/// Produce SourceFile tracked structs for all active workspace files.
/// Salsa GC removes SourceFile entities (and cascades to parsed_doc, file_index,
/// symbol_map, parse_error_count) when they are absent from this function's output.
#[salsa::tracked]
pub fn workspace_files<'db>(db: &'db dyn salsa::Database, ws: Workspace) -> Vec<SourceFile<'db>> {
    ws.files(db)
        .iter()
        .map(|(uri, ft)| SourceFile::new(db, uri.clone(), *ft))
        .collect()
}

/// O(log N) lookup of a single `SourceFile` by URI string.
///
/// `Workspace::files` is kept sorted by URI (see `DocumentStore::sync_workspace_files`),
/// so binary search gives the index in O(log N). `workspace_files` outputs in the
/// same 1:1 order, so the same index addresses both slices.
pub fn find_source_file<'db>(
    db: &'db dyn salsa::Database,
    ws: Workspace,
    uri_str: &str,
) -> Option<SourceFile<'db>> {
    let ws_files = ws.files(db);
    let idx = ws_files
        .binary_search_by(|(u, _)| u.as_ref().cmp(uri_str))
        .ok()?;
    workspace_files(db, ws).into_iter().nth(idx)
}
