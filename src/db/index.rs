//! `file_index` salsa query — derives a compact `FileIndex` from a parsed
//! document. Depends on `parsed_doc`, so editing a file reparses once and the
//! index re-extracts from the new AST.

use std::sync::Arc;

use salsa::Database;

use crate::db::input::SourceFile;
use crate::db::parse::parsed_doc;
use crate::file_index::FileIndex;

/// Arc wrapper for `FileIndex`. Uses structural equality on the inner
/// `FileIndex` so salsa can short-circuit downstream queries (e.g.
/// `workspace_index`) when a body-only edit produces an identical index.
#[derive(Clone, PartialEq, Debug)]
pub struct IndexArc(pub Arc<FileIndex>);

impl IndexArc {
    pub fn get(&self) -> &FileIndex {
        &self.0
    }
}

// SAFETY: writes through `old_pointer` only when returning `true`. Uses
// structural equality on `FileIndex` so that body-only edits (no declaration
// change) return `false` and don't cascade to `workspace_index`.
unsafe impl salsa::Update for IndexArc {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        let old_ref = unsafe { &mut *old_pointer };
        if *old_ref.0 == *new_value.0 {
            false
        } else {
            *old_ref = new_value;
            true
        }
    }
}

/// Build the compact symbol index for a file.  Salsa compares the returned
/// `IndexArc` structurally via `FileIndex::PartialEq`; if declarations are
/// unchanged (body-only edit) the comparison returns equal and `workspace_index`
/// is not re-run.
///
/// Fast path: if the workspace scan seeded a `cached_index` (loaded from the
/// on-disk cache), return it directly — no parse, no extract.
#[salsa::tracked]
pub fn file_index(db: &dyn Database, file: SourceFile) -> IndexArc {
    if let Some(cached) = file.cached_index(db) {
        return IndexArc(cached);
    }
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
            None,
        );
        let idx = file_index(host.db(), file);
        assert_eq!(idx.get().classes.len(), 1);
        assert_eq!(idx.get().classes[0].name, "Foo".into());
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
            None,
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

    #[test]
    fn body_only_edit_produces_equal_index_arc() {
        // Changing only the method body must not alter the FileIndex —
        // verifying this directly is cleaner than counting salsa re-runs and
        // avoids interference with the shared CALLS counter above.
        let mut host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(2),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass Foo { public function bar(): int { return 1; } }"),
            None,
        );

        let before = file_index(host.db(), file);

        // Change the method body only — no declaration-level change.
        file.set_text(host.db_mut()).to(Arc::<str>::from(
            "<?php\nclass Foo { public function bar(): int { return 2; } }",
        ));
        let after = file_index(host.db(), file);

        assert_eq!(
            before, after,
            "body-only edit must produce an equal IndexArc so salsa can short-circuit workspace_index"
        );
    }
}
