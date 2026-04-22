//! Salsa inputs.

use std::sync::Arc;

/// Opaque file identifier used as a stable key for a source file across edits.
/// Backend will map `Url` <-> `FileId`; salsa queries key on `SourceFile` which
/// wraps this id plus the current text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// Per-file salsa input. A new revision is observed whenever `text` is set.
#[salsa::input]
pub struct SourceFile {
    pub id: FileId,
    pub text: Arc<str>,
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
