//! `class_issues` salsa query — runs `mir_analyzer::class::ClassAnalyzer` once
//! per workspace codebase revision and returns all class-level issues across
//! every user file.
//!
//! Class-level checks (circular inheritance, final-class extension, deprecated
//! parent, etc.) require the full codebase — they can't run file-by-file.
//! Keeping them in a single workspace-scoped query means `ClassAnalyzer` runs
//! **once per codebase change** rather than once per file per codebase change,
//! which is the key performance difference versus inlining the call in
//! `semantic_issues`.

use std::collections::HashSet;
use std::sync::Arc;

use mir_issues::Issue;
use salsa::Update;

use crate::db::analysis::LspDatabase;
use crate::db::codebase::codebase;
use crate::db::input::Workspace;

/// All class-level issues for the workspace, computed in one pass.
#[derive(Clone)]
pub struct ClassIssuesArc(pub Arc<[Issue]>);

// SAFETY: identical contract to other `*Arc` newtypes in this module.
unsafe impl Update for ClassIssuesArc {
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

/// Run `ClassAnalyzer` over the workspace codebase once and return every
/// class-level issue. Callers filter to the file they care about.
///
/// Source text is intentionally omitted from the analyzer — the LSP surfaces
/// diagnostics by range, not by snippet, so the `sources` map inside
/// `ClassAnalyzer` is unused here.
#[salsa::tracked(no_eq)]
pub fn class_issues(db: &dyn LspDatabase, ws: Workspace) -> ClassIssuesArc {
    let cb = codebase(db, ws);
    let mir_db = db.cached_mir_db(cb.0.clone(), ws.php_version(db));
    let files = ws.files(db);
    let analyzed_files: HashSet<Arc<str>> = files.iter().map(|f| f.uri(db)).collect();
    let issues = salsa::attach_allow_change(&mir_db, || {
        mir_analyzer::class::ClassAnalyzer::with_files(&mir_db, analyzed_files, &[]).analyze_all()
    });
    ClassIssuesArc(Arc::from(issues))
}
