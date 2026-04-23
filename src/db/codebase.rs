//! `codebase` salsa query — aggregates every file's `StubSlice` into a
//! finalized `mir_codebase::Codebase` via `codebase_from_parts`.
//!
//! When any file's `file_definitions` output changes, salsa marks this query
//! dirty and re-runs it. Re-running calls `file_definitions` for each file in
//! the workspace; unchanged files return their memoized slice instantly, so
//! the work per edit is `O(N * merge_cost)` (plus one `finalize()`).
//!
//! Phase C step 2: query exists and has correctness tests; Backend still uses
//! the imperative `remove/collect/finalize` path. Step 3 migrates Backend.

use std::sync::Arc;

use mir_codebase::Codebase;
use salsa::{Database, Update};

use crate::db::definitions::file_definitions;
use crate::db::input::Workspace;

/// Opaque handle to a finalized Codebase. `Arc::ptr_eq` for the `Update`
/// contract — every re-run produces a new `Arc`, matching `ParsedArc`'s
/// pattern.
#[derive(Clone)]
pub struct CodebaseArc(pub Arc<Codebase>);

impl CodebaseArc {
    #[allow(dead_code)] // reserved for backend migration (step 3).
    pub fn get(&self) -> &Codebase {
        &self.0
    }
}

// SAFETY: identical contract to other `*Arc` newtypes in this module.
unsafe impl Update for CodebaseArc {
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

/// Build a finalized Codebase from the bundled PHP stubs (string/array/etc.
/// builtins) plus every user file's `StubSlice`. Depends on `Workspace.files`
/// and transitively on every file's `file_definitions` query. Stubs are
/// treated as constant — they don't participate in salsa invalidation.
///
/// Load order matches today's imperative path (`Backend::new`): stubs first,
/// user definitions second — so user classes with an FQN matching a stub
/// overwrite the stub entry. `finalize()` runs once at the end.
#[salsa::tracked(no_eq)]
pub fn codebase(db: &dyn Database, ws: Workspace) -> CodebaseArc {
    let mut builder = mir_codebase::CodebaseBuilder::new();
    mir_analyzer::stubs::load_stubs(builder.codebase());
    let files = ws.files(db);
    for sf in files.iter() {
        builder.add((*file_definitions(db, *sf).0).clone());
    }
    // TODO: when PHP-version-dependent stubs land, thread a PhpVersion input
    // through this query so different versions don't share memoization.
    CodebaseArc(Arc::new(builder.finalize()))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileId, SourceFile};
    use salsa::Setter;

    #[test]
    fn codebase_aggregates_classes_across_files() {
        let host = AnalysisHost::new();
        let f1 = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///a.php"),
            Arc::<str>::from("<?php\nnamespace A;\nclass Foo {}"),
            None,
        );
        let f2 = SourceFile::new(
            host.db(),
            FileId(1),
            Arc::<str>::from("file:///b.php"),
            Arc::<str>::from("<?php\nnamespace B;\nclass Bar {}"),
            None,
        );
        let ws = Workspace::new(host.db(), Arc::from([f1, f2]));

        let cb = codebase(host.db(), ws);
        assert!(cb.get().type_exists("A\\Foo"));
        assert!(cb.get().type_exists("B\\Bar"));
    }

    #[test]
    fn codebase_reruns_after_file_edit() {
        let mut host = AnalysisHost::new();
        let f1 = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass Before {}"),
            None,
        );
        let ws = Workspace::new(host.db(), Arc::from([f1]));

        let a1 = codebase(host.db(), ws);
        assert!(a1.get().type_exists("Before"));
        let first_ptr = Arc::as_ptr(&a1.0);

        f1.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\nclass After {}"));
        let a2 = codebase(host.db(), ws);
        assert_ne!(first_ptr, Arc::as_ptr(&a2.0), "edit should invalidate");
        assert!(a2.get().type_exists("After"));
        assert!(!a2.get().type_exists("Before"));
    }

    #[test]
    fn codebase_memoizes_when_nothing_changes() {
        let host = AnalysisHost::new();
        let f1 = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///t.php"),
            Arc::<str>::from("<?php\nclass X {}"),
            None,
        );
        let ws = Workspace::new(host.db(), Arc::from([f1]));

        let a1 = codebase(host.db(), ws);
        let a2 = codebase(host.db(), ws);
        assert!(
            Arc::ptr_eq(&a1.0, &a2.0),
            "no input change — second call should return the memoized Arc"
        );
    }
}
