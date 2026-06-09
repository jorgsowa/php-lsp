//! Gate-1 verification: does salsa 0.26.x actually free memo heap storage when
//! a `#[salsa::tracked]` struct is deleted?
//!
//! The plan (docs/salsa-gc-plan.md) requires this to be true before the L2
//! tracked-struct migration is worth attempting. If salsa only marks memos
//! stale without freeing their heap, converting `SourceFile` to
//! `#[salsa::tracked]` would not reclaim memory.
//!
//! Method: count `DidDiscard` events via `Storage::event_callback`. A discard
//! event fires when salsa drops a memo Box, returning heap to the allocator.
//! The test creates N tracked structs + downstream memos, reduces to N-1,
//! forces the creator query to re-execute, and asserts at least one discard
//! occurred.

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use salsa::{Database, EventKind, Setter};

    // ── Minimal test database with event callback ─────────────────────────

    #[salsa::db]
    struct GcTestDb {
        storage: salsa::Storage<Self>,
    }

    #[salsa::db]
    impl Database for GcTestDb {}

    impl GcTestDb {
        fn with_discard_counter(counter: Arc<AtomicUsize>) -> Self {
            let storage = salsa::StorageHandle::<Self>::new(Some(Box::new(move |event| {
                if matches!(event.kind, EventKind::DidDiscard { .. }) {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            })))
            .into_storage();
            Self { storage }
        }
    }

    // ── Minimal tracked struct + creator + downstream query ───────────────

    /// Input: how many files are "active".
    #[salsa::input]
    struct ActiveFileCount {
        n: u32,
    }

    /// A tracked struct representing one workspace file.
    /// In salsa 0.26.x, all non-`#[tracked]` fields are identity ("id") fields.
    /// `file_id` is an identity field, so reducing n from 3 to 2 makes the
    /// struct with file_id=2 genuinely stale (not merely positionally dropped).
    #[salsa::tracked]
    struct TrackedFile<'db> {
        file_id: u32,
    }

    /// Creator: produces `n` TrackedFile instances. Salsa diffs old vs new
    /// outputs on re-execution; any struct absent from the new output is
    /// deleted via `remove_stale_output` → `delete_entity` → `DidDiscard`.
    #[salsa::tracked]
    fn active_files<'db>(db: &'db dyn Database, count: ActiveFileCount) -> Vec<TrackedFile<'db>> {
        (0..count.n(db))
            .map(|id| TrackedFile::new(db, id))
            .collect()
    }

    /// Downstream query keyed on a TrackedFile. When TrackedFile(file_id=2) is
    /// deleted, its memo table is cleared and `DidDiscard` fires for this entry.
    #[salsa::tracked]
    fn file_hash<'db>(db: &'db dyn Database, file: TrackedFile<'db>) -> u64 {
        file.file_id(db) as u64 * 0xdeadbeef
    }

    // ── Tests ─────────────────────────────────────────────────────────────

    /// L2-gate-1 (lsp / salsa 0.26.x): tracked-struct deletion fires DidDiscard.
    ///
    /// Pass condition: at least one `DidDiscard` event is observed after the
    /// creator query removes one output struct. This proves that memo heap
    /// memory is returned to the allocator (not merely marked stale), making
    /// the L2 tracked-struct migration worthwhile.
    #[test]
    fn tracked_struct_deletion_fires_discard_event() {
        let discard_count = Arc::new(AtomicUsize::new(0));
        let mut db = GcTestDb::with_discard_counter(Arc::clone(&discard_count));

        // Phase 1: 3 active files — materialise memos for all of them.
        let count_input = ActiveFileCount::new(&db, 3);
        {
            let files = active_files(&db, count_input);
            for &f in &files {
                let _ = file_hash(&db, f);
            }
        } // `files` dropped here; &db borrow released

        assert_eq!(
            discard_count.load(Ordering::Relaxed),
            0,
            "no discards before any revision change"
        );

        // Phase 2: reduce to 2 active files (bumps salsa revision).
        count_input.set_n(&mut db).to(2);

        // Re-running the creator triggers diff_outputs → remove_stale_output
        // → delete_entity → clear_memos → DidDiscard for TrackedFile(file_id=2)
        // and its downstream file_hash memo.
        let _ = active_files(&db, count_input);

        let discards = discard_count.load(Ordering::Relaxed);
        assert!(
            discards > 0,
            "salsa 0.26.x must fire DidDiscard when a tracked struct is deleted \
             (L2-gate-1); got 0 discards — tracked-struct GC does not free memory \
             and the L2 migration would be a no-op"
        );
    }
}
