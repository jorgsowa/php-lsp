//! The `parsed_doc` salsa query: parses a `SourceFile` into an `Arc<ParsedDoc>`
//! under salsa memoization. Downstream queries (file_index, method_returns,
//! semantic diagnostics) depend on this one, so each file is parsed at most
//! once per revision.
//!
//! `ParsedDoc` owns a self-referential bumpalo arena and cannot safely
//! implement the structural `Update` trait — instead we wrap in a `ParsedArc`
//! newtype whose `Update` impl uses `Arc::ptr_eq`. Every reparse produces a
//! new `Arc`, so pointer equality is a correct (if conservative) "changed"
//! signal: salsa never falsely backdates, and downstream queries re-run after
//! every input text change.

use std::sync::Arc;

use salsa::{Database, Update};

use crate::ast::ParsedDoc;
use crate::db::input::SourceFile;
use crate::diagnostics::parse_document;

/// Opaque handle to a parsed document. Cheap to clone (refcount bump); never
/// compared structurally. See module docs for the `Update` contract.
///
/// No `Debug` impl because `ParsedDoc` isn't `Debug` (it owns raw pointers
/// into a bumpalo arena). Salsa doesn't require `Debug` on tracked returns
/// when `no_eq` is used.
#[derive(Clone)]
pub struct ParsedArc(pub Arc<ParsedDoc>);

impl ParsedArc {
    pub fn get(&self) -> &ParsedDoc {
        &self.0
    }
}

// SAFETY: `maybe_update` writes `new` through `old_pointer` exactly when it
// returns `true`. The `ptr_eq` short-circuit returns `false` without writing,
// matching salsa's "no observable change" contract. `ParsedDoc` is already
// `Send + Sync` (see `ast.rs:98`).
unsafe impl Update for ParsedArc {
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

/// Parse the file's source text. `no_eq` because `ParsedArc` has no
/// structural equality — invalidation is driven entirely by input changes,
/// not by comparing the new value against the old one.
///
/// Phase F: `lru = 2048` bounds the number of cached ASTs. Parsed docs own
/// bumpalo arenas and are the largest memoized values in the db; dropping
/// older entries caps resident memory at roughly 2048 × avg_ast_size.
/// Re-reads after eviction reparse from the live `SourceFile::text` input
/// (cheap `Arc<str>` clone). This replaces the hand-written
/// `DocumentStore::indexed_order` LRU that used to bound `Document` entries.
#[salsa::tracked(no_eq, lru = 2048)]
pub fn parsed_doc(db: &dyn Database, file: SourceFile) -> ParsedArc {
    let text = file.text(db);
    let (doc, _diags) = parse_document(&text);
    ParsedArc(Arc::new(doc))
}

/// Parse-error count, derived from `parsed_doc`. Kept as a separate query so
/// callers that only need the diagnostic count don't clone the parsed AST.
#[salsa::tracked]
pub fn parse_error_count(db: &dyn Database, file: SourceFile) -> usize {
    let text = file.text(db);
    let (_doc, diags) = parse_document(&text);
    diags.len()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileId, SourceFile};
    use salsa::Setter;

    static CALLS: AtomicUsize = AtomicUsize::new(0);

    #[salsa::tracked]
    fn counted_parse(db: &dyn Database, file: SourceFile) -> usize {
        CALLS.fetch_add(1, Ordering::SeqCst);
        parsed_doc(db, file).get().errors.len()
    }

    #[test]
    fn parsed_doc_returns_ast() {
        let host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nfunction greet() {}"),
        );
        let arc = parsed_doc(host.db(), file);
        assert!(arc.get().errors.is_empty());
        assert!(!arc.get().program().stmts.is_empty());
    }

    #[test]
    fn parsed_doc_memoizes_and_invalidates() {
        CALLS.store(0, Ordering::SeqCst);
        let mut host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(1),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nfunction a() {}"),
        );

        let _ = counted_parse(host.db(), file);
        let _ = counted_parse(host.db(), file);
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            1,
            "salsa should memoize the second call with unchanged input"
        );

        file.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\nclass {"));
        let _ = counted_parse(host.db(), file);
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "downstream query should re-run after input text changes"
        );
    }

    #[test]
    fn parse_error_count_reflects_diagnostics() {
        let host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(2),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass {"),
        );
        assert!(parse_error_count(host.db(), file) > 0);
    }
}
