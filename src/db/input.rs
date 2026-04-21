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
