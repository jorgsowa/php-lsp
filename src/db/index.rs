//! `file_index` salsa query — derives a compact `FileIndex` from a parsed
//! document. Depends on `parsed_doc`, so editing a file reparses once and the
//! index re-extracts from the new AST.

use std::sync::Arc;

use salsa::{Database, Update};

use crate::db::input::SourceFile;
use crate::db::parse::parsed_doc;
use crate::file_index::FileIndex;

/// Arc wrapper for `FileIndex`. `FileIndex` is structurally clone-able but
/// doesn't implement salsa's `Update` — we wrap in `Arc` and compare pointers,
/// mirroring the `ParsedArc` approach. A new extract always produces a fresh
/// `Arc`, so pointer inequality is a safe "changed" signal.
#[derive(Clone)]
pub struct IndexArc(pub Arc<FileIndex>);

impl IndexArc {
    #[allow(dead_code)] // Used by tests and by Phase E call sites.
    pub fn get(&self) -> &FileIndex {
        &self.0
    }
}

// SAFETY: same contract as `ParsedArc::maybe_update` — only writes through
// `old_pointer` when returning `true`. `FileIndex` is `Send + Sync` by virtue
// of its fields (all owned `String`/`Vec`).
unsafe impl Update for IndexArc {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        let old_ref = unsafe { &mut *old_pointer };
        if Arc::ptr_eq(&old_ref.0, &new_value.0) {
            false
        } else {
            *old_ref = new_value;
            true
        }
    }
}

/// Build the compact symbol index for a file. `no_eq` so salsa doesn't try to
/// compare `IndexArc` structurally; invalidation flows from `parsed_doc`.
#[salsa::tracked(no_eq)]
pub fn file_index(db: &dyn Database, file: SourceFile) -> IndexArc {
    let doc = parsed_doc(db, file);
    IndexArc(Arc::new(FileIndex::extract(doc.get())))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileId, SourceFile};
    use crate::db::parse::parsed_doc;
    use salsa::Setter;

    static CALLS: AtomicUsize = AtomicUsize::new(0);

    /// Wrap `file_index` with a counter to verify salsa shares the `parsed_doc`
    /// memoization between `file_index` and other downstream queries.
    #[salsa::tracked]
    fn counted_index_len(db: &dyn Database, file: SourceFile) -> usize {
        CALLS.fetch_add(1, Ordering::SeqCst);
        file_index(db, file).get().classes.len()
    }

    #[test]
    fn file_index_extracts_class() {
        let host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass Foo { public function bar() {} }"),
        );
        let idx = file_index(host.db(), file);
        assert_eq!(idx.get().classes.len(), 1);
        assert_eq!(idx.get().classes[0].name, "Foo");
    }

    #[test]
    fn file_index_memoizes_and_shares_parse_with_downstream() {
        CALLS.store(0, Ordering::SeqCst);
        let mut host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(1),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass A {} class B {}"),
        );

        // Fetch the parsed doc, then the index — salsa should parse once.
        let _ = parsed_doc(host.db(), file);
        let _ = counted_index_len(host.db(), file);
        let _ = counted_index_len(host.db(), file);
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            1,
            "index query should memoize within a revision"
        );

        // Edit the file — both the parse and the index should re-run.
        file.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\nclass A {}"));
        let _ = counted_index_len(host.db(), file);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);

        let idx = file_index(host.db(), file);
        assert_eq!(idx.get().classes.len(), 1);
    }
}
