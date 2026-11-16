//! `file_index` salsa query — derives a compact `FileIndex` from a parsed
//! document. Depends on `parsed_doc`, so editing a file reparses once and the
//! index re-extracts from the new AST.

use std::sync::Arc;

use crate::index::file_index::FileIndex;

/// Arc wrapper for `FileIndex`. Uses structural equality on the inner
/// `FileIndex` so salsa can short-circuit downstream queries (e.g.
/// `workspace_index`) when a body-only edit produces an identical index.
#[derive(Clone, PartialEq, Debug)]
pub struct IndexArc(pub Arc<FileIndex>);
