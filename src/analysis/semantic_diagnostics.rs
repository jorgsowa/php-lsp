/// Semantic diagnostics bridge.
///
/// Delegates all analysis to the `mir-analyzer` crate and converts its `Issue`
/// type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Uri,
};

use crate::analysis::diagnostics::PHP_LSP_SOURCE;
use crate::document::ast::ParsedDoc;
use crate::lang::config::DiagnosticsConfig;

/// Run semantic checks on `doc` against the supplied `AnalysisSession`.
/// Ingests the current file, runs Pass 2 via `FileAnalyzer`, and returns LSP
/// diagnostics filtered by `DiagnosticsConfig`.
pub fn semantic_diagnostics(
    uri: &Uri,
    doc: &ParsedDoc,
    session: &mir_analyzer::AnalysisSession,
    cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }
    let file: std::sync::Arc<str> = std::sync::Arc::from(uri.as_str());
    session.ingest_file(file.clone(), doc.source_arc());
    let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());
    let owned_program = php_ast::owned::to_owned_program(doc.program());
    let analyzer = mir_analyzer::FileAnalyzer::new(session);
    let analysis = analyzer.analyze(file.clone(), doc.source(), &owned_program, &source_map);
    let class_issues = session.class_issues(std::slice::from_ref(&file));
    analysis
        .issues
        .into_iter()
        .chain(class_issues)
        .filter(|i| !i.suppressed)
        .filter(|i| issue_passes_filter(i, cfg))
        .map(to_lsp_diagnostic)
        .collect()
}

/// Convert pre-computed raw issues (from `db::semantic::semantic_issues`) into
/// LSP diagnostics, applying the user's `DiagnosticsConfig` filter. Keeping
/// filter + conversion outside the salsa query preserves memoization across
/// config toggles (the user flipping a category must not rerun the analyzer).
///
/// `ParseError` issues are always excluded here, regardless of which mir API
/// produced `issues`: php-lsp already surfaces raw parse errors as
/// `SyntaxError` diagnostics from its own parser pass (see
/// `crate::analysis::diagnostics::diagnostics_from_doc`), so this is the one
/// place every issue source funnels through before becoming a `Diagnostic`.
pub fn issues_to_diagnostics(
    issues: &[mir_issues::Issue],
    _uri: &Uri,
    cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }
    issues
        .iter()
        .filter(|i| !matches!(i.kind, mir_issues::IssueKind::ParseError { .. }))
        .filter(|i| issue_passes_filter(i, cfg))
        .cloned()
        .map(to_lsp_diagnostic)
        .collect()
}

/// Like [`issues_to_diagnostics`], but additionally holds back
/// workspace-symbol-dependent issues (see [`issue_needs_workspace_index`])
/// while `index_ready` is `false`. Skips the extra filter/clone pass
/// entirely once ready — the common case, well past server startup — so
/// this costs nothing in steady state.
pub fn issues_to_diagnostics_gated(
    issues: &[mir_issues::Issue],
    uri: &Uri,
    cfg: &DiagnosticsConfig,
    index_ready: bool,
) -> Vec<Diagnostic> {
    if index_ready {
        return issues_to_diagnostics(issues, uri, cfg);
    }
    let filtered: Vec<mir_issues::Issue> = issues
        .iter()
        .filter(|i| !issue_needs_workspace_index(&i.kind))
        .cloned()
        .collect();
    issues_to_diagnostics(&filtered, uri, cfg)
}

/// Whether resolving this issue required looking up a symbol (class,
/// function, method, or trait) by name against the workspace-wide symbol
/// table, as opposed to something derivable from the file's own contents.
///
/// Before the initial workspace scan finishes, that table only reflects the
/// subset of files indexed so far, so these specific kinds can misreport a
/// symbol declared in a not-yet-scanned file as undefined — see issue #242.
/// Everything else (return-type mismatches, unreachable code, duplicate
/// declarations within a file, etc.) is safe to report immediately: it's
/// checked against the file's own text and can't be invalidated by more of
/// the workspace becoming known.
pub fn issue_needs_workspace_index(kind: &mir_issues::IssueKind) -> bool {
    use mir_issues::IssueKind;
    matches!(
        kind,
        IssueKind::UndefinedFunction { .. }
            | IssueKind::UndefinedMethod { .. }
            | IssueKind::UndefinedClass { .. }
            | IssueKind::UndefinedTrait { .. }
    )
}

/// Returns `true` if the mir-analyzer issue is allowed through by the config.
fn issue_passes_filter(issue: &mir_issues::Issue, cfg: &DiagnosticsConfig) -> bool {
    use mir_issues::IssueKind;
    match &issue.kind {
        IssueKind::UndefinedVariable { .. } | IssueKind::PossiblyUndefinedVariable { .. } => {
            cfg.undefined_variables
        }
        IssueKind::UndefinedFunction { .. } | IssueKind::UndefinedMethod { .. } => {
            cfg.undefined_functions
        }
        IssueKind::UndefinedClass { .. } | IssueKind::UndefinedTrait { .. } => {
            cfg.undefined_classes
        }
        IssueKind::InvalidTraitUse { .. } => cfg.type_errors,
        IssueKind::TooFewArguments { .. }
        | IssueKind::TooManyArguments { .. }
        | IssueKind::InvalidPassByReference { .. }
        | IssueKind::InvalidNamedArgument { .. } => cfg.arity_errors,
        // InvalidArgument covers both arity errors and type mismatches in mir-analyzer;
        // show it if either toggle is on.
        IssueKind::InvalidArgument { .. } | IssueKind::PossiblyInvalidArgument { .. } => {
            cfg.arity_errors || cfg.type_errors
        }
        IssueKind::InvalidReturnType { .. }
        | IssueKind::NullMethodCall { .. }
        | IssueKind::NullPropertyFetch { .. }
        | IssueKind::NullArrayAccess
        | IssueKind::NullArgument { .. }
        | IssueKind::PossiblyNullMethodCall { .. }
        | IssueKind::PossiblyNullPropertyFetch { .. }
        | IssueKind::PossiblyNullArrayAccess
        | IssueKind::PossiblyNullArgument { .. }
        | IssueKind::NullableReturnStatement { .. }
        | IssueKind::InvalidPropertyAssignment { .. }
        | IssueKind::InvalidOperand { .. }
        | IssueKind::InvalidCast { .. }
        | IssueKind::AbstractInstantiation { .. }
        | IssueKind::MixedClone
        | IssueKind::ReadonlyPropertyAssignment { .. }
        | IssueKind::ArgumentTypeCoercion { .. } => cfg.type_errors,
        IssueKind::DeprecatedCall { .. }
        | IssueKind::DeprecatedMethodCall { .. }
        | IssueKind::DeprecatedMethod { .. }
        | IssueKind::DeprecatedClass { .. } => cfg.deprecated_calls,
        IssueKind::CircularInheritance { .. } => cfg.type_errors,
        IssueKind::DuplicateClass { .. }
        | IssueKind::DuplicateInterface { .. }
        | IssueKind::DuplicateTrait { .. }
        | IssueKind::DuplicateEnum { .. }
        | IssueKind::DuplicateFunction { .. } => cfg.duplicate_declarations,
        IssueKind::UnusedVariable { .. }
        | IssueKind::UnusedParam { .. }
        | IssueKind::UnusedMethod { .. }
        | IssueKind::UnusedProperty { .. }
        | IssueKind::UnusedFunction { .. } => cfg.unused_symbols,
        IssueKind::MissingReturnType { .. }
        | IssueKind::MissingParamType { .. }
        | IssueKind::MissingPropertyType { .. } => cfg.missing_types,
        IssueKind::MixedArgument { .. }
        | IssueKind::MixedAssignment { .. }
        | IssueKind::MixedMethodCall { .. }
        | IssueKind::MixedPropertyFetch { .. }
        | IssueKind::MixedPropertyAssignment { .. }
        | IssueKind::MixedArrayAccess
        | IssueKind::MixedArrayOffset
        | IssueKind::MixedReturnStatement { .. } => cfg.mixed_usage,
        IssueKind::DocblockTypeContradiction { .. }
        | IssueKind::UnevaluatedCode { .. }
        | IssueKind::IfThisIsMismatch { .. } => true,
        _ => true,
    }
}

fn to_lsp_diagnostic(issue: mir_issues::Issue) -> Diagnostic {
    // mir uses 1-based lines; LSP uses 0-based. Columns are 0-based on both sides.
    let line = issue.location.line.saturating_sub(1);
    let col_start = issue.location.col_start as u32;
    let col_end = issue.location.col_end as u32;
    Diagnostic {
        range: Range {
            start: Position {
                line,
                character: col_start,
            },
            end: Position {
                line,
                character: col_end.max(col_start + 1),
            },
        },
        severity: Some(match issue.severity {
            mir_issues::Severity::Error => DiagnosticSeverity::ERROR,
            mir_issues::Severity::Warning => DiagnosticSeverity::WARNING,
            mir_issues::Severity::Info => DiagnosticSeverity::INFORMATION,
        }),
        code: Some(NumberOrString::String(issue.kind.name().to_string())),
        source: Some(PHP_LSP_SOURCE.to_string()),
        message: issue.kind.message(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_lsp_diagnostic_sets_code_to_issue_kind_name() {
        use mir_issues::{Issue, IssueKind, Location};
        use std::sync::Arc;
        use tower_lsp_server::ls_types::NumberOrString;

        let location = Location {
            file: Arc::from("file:///test.php"),
            line: 1,
            line_end: 1,
            col_start: 0,
            col_end: 3,
        };
        let issue = Issue::new(
            IssueKind::UndefinedClass {
                name: "Foo".to_string(),
            },
            location,
        );
        let diag = to_lsp_diagnostic(issue);
        assert_eq!(
            diag.code,
            Some(NumberOrString::String("UndefinedClass".to_string())),
            "diagnostic code must be the IssueKind name so code actions can match by type"
        );
        assert!(
            diag.message.contains("Foo"),
            "diagnostic message should mention the class name"
        );
    }
}
