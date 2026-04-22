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
    #[allow(dead_code)] // reserved for codebase aggregator (step 2).
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
#[salsa::tracked(no_eq)]
pub fn file_definitions(db: &dyn Database, file: SourceFile) -> SliceArc {
    let doc = parsed_doc(db, file);
    let text = file.text(db);
    let file_path: Arc<str> = Arc::from(format!("file:{}", file.id(db).0));
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
            Arc::<str>::from("<?php\nnamespace App;\nclass Foo {}"),
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
        let file = SourceFile::new(host.db(), FileId(1), Arc::<str>::from("<?php\nclass A {}"));
        let a1 = file_definitions(host.db(), file);
        let first_ptr = Arc::as_ptr(&a1.0);

        file.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\nclass B {}"));
        let a2 = file_definitions(host.db(), file);
        assert_ne!(first_ptr, Arc::as_ptr(&a2.0));
        let classes: Vec<&str> = a2.get().classes.iter().map(|c| c.fqcn.as_ref()).collect();
        assert_eq!(classes, vec!["B"]);
    }
}
