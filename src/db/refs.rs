//! `file_refs` / `symbol_refs` salsa queries — Phase D.
//!
//! Replaces the imperative `build_reference_index` scan that ran on workspace
//! startup. References are now computed lazily on first `textDocument/references`
//! call and memoized thereafter. Body-only edits to a single file invalidate
//! only that file's `file_refs`; structural edits also invalidate `codebase(ws)`
//! which cascades into every `file_refs` because StatementsAnalyzer depends on
//! the finalized codebase.

use std::sync::Arc;

use salsa::Update;

use crate::db::analysis::LspDatabase;
use crate::db::codebase::codebase;
use crate::db::input::{SourceFile, Workspace};
use crate::db::parse::parsed_doc;

/// A single Pass-2 reference observed during StatementsAnalyzer.
/// `key` mirrors `Codebase::symbol_reference_locations` keys so that consumers
/// can aggregate by the same scheme `mark_*_referenced_at` would have used.
#[derive(Debug, Clone)]
pub struct FileRefRecord {
    pub key: Arc<str>,
    pub line: u32,
    pub col_start: u16,
    pub col_end: u16,
}

#[derive(Clone)]
pub struct FileRefsArc(pub Arc<Vec<FileRefRecord>>);

impl FileRefsArc {
    pub fn get(&self) -> &[FileRefRecord] {
        &self.0
    }
}

// SAFETY: same contract as other `*Arc` newtypes — `Arc::ptr_eq` is sufficient
// because every re-run of the tracked query allocates a fresh `Arc`.
unsafe impl Update for FileRefsArc {
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

type SymbolRefsInner = Arc<Vec<(Arc<str>, u32, u16, u16)>>;

#[derive(Clone)]
pub struct SymbolRefsArc(pub SymbolRefsInner);

impl SymbolRefsArc {
    pub fn get(&self) -> &[(Arc<str>, u32, u16, u16)] {
        &self.0
    }
}

unsafe impl Update for SymbolRefsArc {
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

/// Run Pass-2 analysis on `file` against the workspace codebase and return
/// every resolved reference with its codebase key and byte span.
///
/// The analyzer internally records reference locations via the salsa
/// accumulator on the cloned `MirDb` — we deliberately ignore those side
/// effects here and build our own aggregation from the resolved-symbol list,
/// keeping the data flow purely functional from salsa's perspective.
#[salsa::tracked(no_eq)]
pub fn file_refs(db: &dyn LspDatabase, ws: Workspace, file: SourceFile) -> FileRefsArc {
    let cb = codebase(db, ws);
    let mir_db = db.cached_mir_db(cb.0.clone(), ws.php_version(db));
    let doc = parsed_doc(db, file);
    let uri = file.uri(db);
    let source = file.text(db);
    let map = php_rs_parser::source_map::SourceMap::new(&source);
    let mut issue_buffer = mir_issues::IssueBuffer::new();
    let mut symbols = Vec::new();
    salsa::attach_allow_change(&mir_db, || {
        let mut analyzer = mir_analyzer::stmt::StatementsAnalyzer::new(
            &mir_db,
            uri,
            &source,
            &map,
            &mut issue_buffer,
            &mut symbols,
            ws.php_version(db),
            false,
        );
        let mut ctx = mir_analyzer::context::Context::new();
        analyzer.analyze_stmts(&doc.get().program().stmts, &mut ctx);
    });

    let records: Vec<FileRefRecord> = symbols
        .into_iter()
        .filter_map(|s| {
            let key = s.codebase_key()?;
            let (line, col_start) = map.offset_to_line_col(s.span.start).to_one_based();
            let (_, col_end) = map.offset_to_line_col(s.span.end).to_one_based();
            Some(FileRefRecord {
                key: Arc::from(key),
                line,
                col_start: col_start.saturating_sub(1) as u16,
                col_end: col_end.saturating_sub(1) as u16,
            })
        })
        .collect();
    FileRefsArc(Arc::new(records))
}

/// Aggregate every file's `file_refs` filtered by `key` into a flat
/// `(uri, start, end)` list — drop-in replacement for
/// `Codebase::get_reference_locations`.
#[salsa::tracked(no_eq)]
pub fn symbol_refs(db: &dyn LspDatabase, ws: Workspace, key: String) -> SymbolRefsArc {
    let files = ws.files(db);
    let mut out: Vec<(Arc<str>, u32, u16, u16)> = Vec::new();
    for sf in files.iter() {
        let refs = file_refs(db, ws, *sf);
        let uri = sf.uri(db);
        for r in refs.get() {
            if r.key.as_ref() == key.as_str() {
                out.push((uri.clone(), r.line, r.col_start, r.col_end));
            }
        }
    }
    SymbolRefsArc(Arc::new(out))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileId, SourceFile};

    #[test]
    fn symbol_refs_finds_function_call_across_files() {
        let host = AnalysisHost::new();
        let f1 = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///a.php"),
            Arc::<str>::from("<?php\nfunction greet(): void {}"),
            None,
        );
        let f2 = SourceFile::new(
            host.db(),
            FileId(1),
            Arc::<str>::from("file:///b.php"),
            Arc::<str>::from("<?php\ngreet();"),
            None,
        );
        let ws = Workspace::new(
            host.db(),
            Arc::from([f1, f2]),
            mir_analyzer::PhpVersion::LATEST,
        );

        let locs = symbol_refs(host.db(), ws, "greet".to_string());
        let found: Vec<&str> = locs.get().iter().map(|(u, _, _, _)| u.as_ref()).collect();
        assert!(
            found.iter().any(|u| *u == "file:///b.php"),
            "expected a reference from b.php, got {:?}",
            found
        );
    }

    #[test]
    fn symbol_refs_memoizes_per_key() {
        let host = AnalysisHost::new();
        let f1 = SourceFile::new(
            host.db(),
            FileId(0),
            Arc::<str>::from("file:///a.php"),
            Arc::<str>::from("<?php\nfunction hi(): void {}\nhi();"),
            None,
        );
        let ws = Workspace::new(host.db(), Arc::from([f1]), mir_analyzer::PhpVersion::LATEST);

        let a = symbol_refs(host.db(), ws, "hi".to_string());
        let b = symbol_refs(host.db(), ws, "hi".to_string());
        assert!(Arc::ptr_eq(&a.0, &b.0), "second call should be memoized");
    }
}
