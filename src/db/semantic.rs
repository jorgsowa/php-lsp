//! `semantic_issues` salsa query — runs `mir_analyzer` Pass-2 body analysis
//! on a single file against the finalized workspace codebase. Depends on
//! `codebase(ws)` and `parsed_doc(file)`, so invalidation happens automatically
//! when the file's text or any file that contributes to the shared codebase
//! changes.
//!
//! The query returns raw `mir_issues::Issue` values. Config-level filtering
//! (`DiagnosticsConfig`) and LSP conversion (`to_lsp_diagnostic`) live outside
//! the query — the user toggling a diagnostic category must not invalidate
//! the expensive analysis.

use std::sync::Arc;

use mir_issues::Issue;
use salsa::{Database, Update};

use crate::db::class_issues::class_issues;
use crate::db::codebase::codebase;
use crate::db::input::{SourceFile, Workspace};
use crate::db::parse::parsed_doc;

/// Opaque handle to the per-file raw issue list. `Arc<[Issue]>` so clones
/// are cheap; `Update` uses `Arc::ptr_eq` like other `*Arc` newtypes in this
/// module.
#[derive(Clone)]
pub struct IssuesArc(pub Arc<[Issue]>);

impl IssuesArc {
    pub fn get(&self) -> &[Issue] {
        &self.0
    }
}

// SAFETY: identical contract to `ParsedArc::maybe_update`.
unsafe impl Update for IssuesArc {
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

/// Run Pass-2 (body analysis) on a single file against the workspace codebase
/// and return the raw issue list with `suppressed` entries already dropped.
///
/// `no_eq` because `IssuesArc` has no structural equality — invalidation is
/// driven by the upstream inputs (codebase, parsed_doc).
#[salsa::tracked(no_eq)]
pub fn semantic_issues(db: &dyn Database, ws: Workspace, file: SourceFile) -> IssuesArc {
    let cb = codebase(db, ws);
    let doc_arc = parsed_doc(db, file);
    let doc = doc_arc.get();
    let uri_arc: Arc<str> = file.uri(db);
    let source = doc.source();
    let source_map = php_rs_parser::source_map::SourceMap::new(source);

    let mut issue_buffer = mir_issues::IssueBuffer::new();
    let mut symbols = Vec::new();
    let php_version = ws.php_version(db);
    let mut analyzer = mir_analyzer::stmt::StatementsAnalyzer::new(
        cb.get(),
        uri_arc.clone(),
        source,
        &source_map,
        &mut issue_buffer,
        &mut symbols,
        php_version,
        false,
    );
    let mut ctx = mir_analyzer::context::Context::new();
    analyzer.analyze_stmts(&doc.program().stmts, &mut ctx);

    let ws_class_issues = class_issues(db, ws);
    let file_class_issues = ws_class_issues
        .0
        .iter()
        .filter(|i| i.location.file == uri_arc)
        .cloned();

    let issues: Vec<Issue> = issue_buffer
        .into_issues()
        .into_iter()
        .chain(file_class_issues)
        .filter(|i| !i.suppressed)
        .collect();
    IssuesArc(Arc::from(issues))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileId, SourceFile};
    use salsa::Setter;

    fn new_file(host: &AnalysisHost, id: u32, uri: &str, src: &str) -> SourceFile {
        SourceFile::new(
            host.db(),
            FileId(id),
            Arc::<str>::from(uri),
            Arc::<str>::from(src),
            None,
        )
    }

    #[test]
    fn semantic_issues_flags_undefined_function() {
        let host = AnalysisHost::new();
        let file = new_file(&host, 0, "file:///a.php", "<?php\nfoo_bar_baz();");
        let ws = Workspace::new(
            host.db(),
            Arc::from([file]),
            mir_analyzer::PhpVersion::LATEST,
        );
        let issues = semantic_issues(host.db(), ws, file);
        assert!(
            issues
                .get()
                .iter()
                .any(|i| matches!(i.kind, mir_issues::IssueKind::UndefinedFunction { .. })),
            "expected an UndefinedFunction issue, got {:?}",
            issues.get()
        );
    }

    #[test]
    fn semantic_issues_memoizes_across_calls() {
        let host = AnalysisHost::new();
        let file = new_file(&host, 0, "file:///a.php", "<?php\nfoo_bar_baz();");
        let ws = Workspace::new(
            host.db(),
            Arc::from([file]),
            mir_analyzer::PhpVersion::LATEST,
        );
        let a = semantic_issues(host.db(), ws, file);
        let b = semantic_issues(host.db(), ws, file);
        assert!(
            Arc::ptr_eq(&a.0, &b.0),
            "second call with unchanged inputs should return the memoized Arc"
        );
    }

    /// When a dependency is absent from the workspace (background scan hasn't
    /// reached it yet), UndefinedClass is emitted at the salsa layer. The LSP
    /// fixes this via PSR-4 lazy-loading before `semantic_issues` runs.
    #[test]
    fn use_imported_class_absent_from_workspace_emits_undefined_class() {
        let host = AnalysisHost::new();
        let consuming = new_file(
            &host,
            0,
            "file:///src/Service/Handler.php",
            "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity;\nfunction handle(): void { $e = new Entity(); }",
        );
        let ws = Workspace::new(
            host.db(),
            Arc::from([consuming]),
            mir_analyzer::PhpVersion::LATEST,
        );
        let issues = semantic_issues(host.db(), ws, consuming);
        assert!(
            issues
                .get()
                .iter()
                .any(|i| matches!(i.kind, mir_issues::IssueKind::UndefinedClass { .. })),
            "expected UndefinedClass when dependency is absent from workspace; got: {:?}",
            issues.get()
        );
    }

    /// Regression: `new Alias()` must not emit UndefinedClass when the aliased
    /// class is present in the workspace. Requires mir 0.14.0+ which populates
    /// `Codebase.file_imports` from `StubSlice.imports`.
    #[test]
    fn new_expr_with_use_alias_resolved_in_workspace() {
        let host = AnalysisHost::new();
        let entity = new_file(
            &host,
            0,
            "file:///src/Model/Entity.php",
            "<?php\nnamespace App\\Model;\nclass Entity {}",
        );
        let handler = new_file(
            &host,
            1,
            "file:///src/Service/Handler.php",
            "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity;\nfunction handle(): void { $e = new Entity(); }",
        );
        let ws = Workspace::new(
            host.db(),
            Arc::from([entity, handler]),
            mir_analyzer::PhpVersion::LATEST,
        );
        let issues = semantic_issues(host.db(), ws, handler);
        let undef: Vec<_> = issues
            .get()
            .iter()
            .filter(|i| matches!(i.kind, mir_issues::IssueKind::UndefinedClass { .. }))
            .collect();
        assert!(
            undef.is_empty(),
            "new Alias() must not emit UndefinedClass when class is in workspace; got: {undef:?}"
        );
    }

    #[test]
    fn semantic_issues_reruns_after_edit() {
        let mut host = AnalysisHost::new();
        let file = new_file(&host, 0, "file:///a.php", "<?php\nfoo_bar_baz();");
        let ws = Workspace::new(
            host.db(),
            Arc::from([file]),
            mir_analyzer::PhpVersion::LATEST,
        );
        let a = semantic_issues(host.db(), ws, file);
        let first_ptr = Arc::as_ptr(&a.0);
        file.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\necho 1;"));
        let b = semantic_issues(host.db(), ws, file);
        assert_ne!(
            first_ptr,
            Arc::as_ptr(&b.0),
            "edit should invalidate memoized issues"
        );
    }
}
