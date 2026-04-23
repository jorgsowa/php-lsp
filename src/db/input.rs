//! Salsa inputs.

use std::sync::Arc;

use mir_codebase::storage::StubSlice;

/// Opaque file identifier used as a stable key for a source file across edits.
/// Backend will map `Url` <-> `FileId`; salsa queries key on `SourceFile` which
/// wraps this id plus the current text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// Per-file salsa input. A new revision is observed whenever `text` is set.
///
/// `uri` is the LSP document URI as a string (e.g. `file:///path/Foo.php`).
/// It lives on the input so that tracked queries like `file_refs` can emit
/// per-symbol location records keyed by URI without needing a separate
/// FileId→URI map outside salsa.
///
/// `cached_slice` (Phase K2): when `Some`, holds a pre-computed `StubSlice`
/// loaded from the on-disk cache. `file_definitions` checks this field
/// first and returns the cached slice instead of parsing + running
/// `DefinitionCollector`. Cleared back to `None` on any text edit — see
/// `DocumentStore::mirror_text` — so a stale cached slice cannot mask a
/// real change. Seeded by workspace scan via
/// `DocumentStore::seed_cached_slice` before the first `file_definitions`
/// call for that file.
#[salsa::input]
pub struct SourceFile {
    pub id: FileId,
    pub uri: Arc<str>,
    pub text: Arc<str>,
    pub cached_slice: Option<Arc<StubSlice>>,
}

/// Workspace-level input: the set of files that participate in whole-program
/// analyses (codebase, references). Updated by the backend when files are
/// discovered (workspace scan, did_open on previously-unseen file) or removed
/// (watched-files delete).
///
/// Uses `durability = HIGH` conceptually — the file list changes rarely
/// (workspace scan, deletions), not on every edit. Salsa's default durability
/// is LOW; backend can opt into HIGH via `set_files` if churn becomes an issue.
#[salsa::input]
pub struct Workspace {
    pub files: Arc<[SourceFile]>,
}
