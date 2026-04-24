//! `file_definitions` salsa query — runs `DefinitionCollector::collect_slice`
//! under salsa memoization, producing a pure `StubSlice` value per file.
//!
//! This is Phase C step 1: the per-file Pass-1 definitions become a
//! tracked query. Phase C step 2 will add a `codebase(Workspace)`
//! aggregator that folds all slices via `mir_codebase::codebase_from_parts`.

use std::sync::Arc;

use mir_codebase::storage::StubSlice;
use salsa::{Database, Update};

use crate::db::input::SourceFile;
use crate::db::parse::parsed_doc;

#[derive(Clone)]
pub struct SliceArc(pub Arc<StubSlice>);

impl SliceArc {
    pub fn get(&self) -> &StubSlice {
        &self.0
    }
}

// SAFETY: identical contract to `ParsedArc::maybe_update`.
unsafe impl Update for SliceArc {
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

/// Collect Pass-1 definitions (classes, interfaces, traits, enums, functions,
/// constants, global_vars) from a file. Uses mir-analyzer's collector in its
/// new pure-slice mode (`collect_slice`), added in mir 0.7.1 for salsa
/// integration.
///
/// Phase K2: when the `cached_slice` input field is `Some`, returns that
/// slice directly and skips parse + `DefinitionCollector`. The slice is
/// seeded before any query runs (by the workspace-scan warm-start path)
/// and cleared on every text edit in `DocumentStore::mirror_text`, so by
/// construction a cached slice is equivalent to what parse+collect would
/// produce for the file's current text. Reading `file.text(db)` on the
/// cached path is still required: it declares a salsa dependency on the
/// text input so that a future edit invalidates this query's memo and
/// forces a recompute via the fresh-parse branch.
#[salsa::tracked(no_eq)]
pub fn file_definitions(db: &dyn Database, file: SourceFile) -> SliceArc {
    // Fast path: serve from the on-disk cache if it was seeded for this
    // file. Still touch `file.text` so salsa invalidates this memo when
    // the editor edits the file — at which point `mirror_text` also
    // clears `cached_slice` back to `None`, forcing the slow path below.
    if let Some(cached) = file.cached_slice(db) {
        let _ = file.text(db);
        return SliceArc(cached);
    }

    let doc = parsed_doc(db, file);
    let text = file.text(db);
    let file_path: Arc<str> = file.uri(db);
    let source_map = php_rs_parser::source_map::SourceMap::new(&text);
    let collector =
        mir_analyzer::collector::DefinitionCollector::new_for_slice(file_path, &text, &source_map);
    let (slice, _issues) = collector.collect_slice(doc.get().program());
    SliceArc(Arc::new(slice))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileId, SourceFile};
    use salsa::Setter;

    #[test]
    fn file_definitions_extracts_class() {
        let host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nnamespace App;\nclass Foo {}"),
            None,
        );
        let slice = file_definitions(host.db(), file);
        let classes: Vec<&str> = slice
            .get()
            .classes
            .iter()
            .map(|c| c.fqcn.as_ref())
            .collect();
        assert_eq!(classes, vec!["App\\Foo"]);
    }

    #[test]
    fn file_definitions_reruns_after_edit() {
        let mut host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(1),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass A {}"),
            None,
        );
        let a1 = file_definitions(host.db(), file);
        let first_ptr = Arc::as_ptr(&a1.0);

        file.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\nclass B {}"));
        let a2 = file_definitions(host.db(), file);
        assert_ne!(first_ptr, Arc::as_ptr(&a2.0));
        let classes: Vec<&str> = a2.get().classes.iter().map(|c| c.fqcn.as_ref()).collect();
        assert_eq!(classes, vec!["B"]);
    }

    /// Phase K2: a pre-seeded `cached_slice` short-circuits parse +
    /// `DefinitionCollector`. The query returns the *cached* slice verbatim
    /// even when the file's text says something different — the scan path
    /// is responsible for keeping them in sync, and this test proves the
    /// fast-path is actually taken (not silently falling through to parse).
    #[test]
    fn file_definitions_returns_seeded_slice_without_parsing() {
        let mut host = AnalysisHost::new();
        // The text says "class Text" but we'll seed a slice claiming "class Cached".
        // A correct fast-path returns Cached; a broken fast-path (ignoring
        // cached_slice and re-parsing) returns Text.
        let file = SourceFile::new(
            host.db(),
            FileId(2),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass Text {}"),
            None,
        );

        let seeded = {
            let mut slice = mir_codebase::storage::StubSlice::default();
            // Build a plausible class entry using the real collector on
            // a file that contains "class Cached" so the StubSlice is
            // well-formed. This is load-bearing: a hand-rolled slice is
            // more likely to drift with mir-codebase schema changes.
            let src = "<?php\nclass Cached {}";
            let source_map = php_rs_parser::source_map::SourceMap::new(src);
            let (doc, _) = crate::diagnostics::parse_document(src);
            let collector = mir_analyzer::collector::DefinitionCollector::new_for_slice(
                Arc::<str>::from("file:///t.php"),
                src,
                &source_map,
            );
            let (s, _) = collector.collect_slice(doc.program());
            slice = s;
            Arc::new(slice)
        };
        file.set_cached_slice(host.db_mut()).to(Some(seeded));

        let out = file_definitions(host.db(), file);
        let classes: Vec<&str> = out.get().classes.iter().map(|c| c.fqcn.as_ref()).collect();
        assert_eq!(
            classes,
            vec!["Cached"],
            "seeded cached_slice must short-circuit parse + collect"
        );
    }

    /// Editing the text after seeding must invalidate the cache — the next
    /// query re-parses from scratch. This is the correctness guarantee that
    /// makes the fast path safe even in the face of editor edits.
    #[test]
    fn edit_invalidates_seeded_slice() {
        let mut host = AnalysisHost::new();
        let file = SourceFile::new(
            host.db(),
            FileId(3),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass Original {}"),
            None,
        );

        // Seed a slice with a misleading fact.
        let misleading = {
            let src = "<?php\nclass Misleading {}";
            let source_map = php_rs_parser::source_map::SourceMap::new(src);
            let (doc, _) = crate::diagnostics::parse_document(src);
            let collector = mir_analyzer::collector::DefinitionCollector::new_for_slice(
                Arc::<str>::from("file:///t.php"),
                src,
                &source_map,
            );
            let (s, _) = collector.collect_slice(doc.program());
            Arc::new(s)
        };
        file.set_cached_slice(host.db_mut()).to(Some(misleading));

        let out1 = file_definitions(host.db(), file);
        let names: Vec<&str> = out1.get().classes.iter().map(|c| c.fqcn.as_ref()).collect();
        assert_eq!(names, vec!["Misleading"]);

        // Editor edit: text changes AND the mirror layer should clear
        // cached_slice (simulating DocumentStore::mirror_text). We do
        // both steps explicitly here to model the production sequence.
        file.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\nclass Edited {}"));
        file.set_cached_slice(host.db_mut()).to(None);

        let out2 = file_definitions(host.db(), file);
        let names: Vec<&str> = out2.get().classes.iter().map(|c| c.fqcn.as_ref()).collect();
        assert_eq!(
            names,
            vec!["Edited"],
            "edit must invalidate cached slice — fresh parse of new text"
        );
    }
}
