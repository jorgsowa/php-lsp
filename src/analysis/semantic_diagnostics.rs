/// Semantic diagnostics bridge.
///
/// Delegates all analysis to the `mir-analyzer` crate and converts its `Issue`
/// type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::analysis::diagnostics::PHP_LSP_SOURCE;
use crate::ast::ParsedDoc;
use crate::config::DiagnosticsConfig;

/// Run semantic checks on `doc` against the supplied `AnalysisSession`.
///
/// Replaces the legacy MirDb-mutating path (pre mir 0.22). The session owns
/// the workspace MirDb internally; this function ingests the current file,
/// runs Pass 2 via `FileAnalyzer`, and returns LSP diagnostics filtered by
/// `DiagnosticsConfig`.
pub fn semantic_diagnostics(
    uri: &Url,
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
pub fn issues_to_diagnostics(
    issues: &[mir_issues::Issue],
    _uri: &Url,
    cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }
    issues
        .iter()
        .filter(|i| issue_passes_filter(i, cfg))
        .cloned()
        .map(to_lsp_diagnostic)
        .collect()
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
        | IssueKind::MixedClone => cfg.type_errors,
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
        // mir 0.22 unused-symbol warnings. Off by default; opt in via
        // `diagnostics.unusedSymbols` in initializationOptions.
        IssueKind::UnusedVariable { .. }
        | IssueKind::UnusedParam { .. }
        | IssueKind::UnusedMethod { .. }
        | IssueKind::UnusedProperty { .. }
        | IssueKind::UnusedFunction { .. } => cfg.unused_symbols,
        // mir 0.36 missing-type-annotation lints. Off by default; opt in via
        // `diagnostics.missingTypes`.
        IssueKind::MissingReturnType { .. }
        | IssueKind::MissingParamType { .. }
        | IssueKind::MissingPropertyType { .. } => cfg.missing_types,
        // mir 0.36 mixed-type usage lints. Off by default; opt in via
        // `diagnostics.mixedUsage`.
        IssueKind::MixedArgument { .. }
        | IssueKind::MixedAssignment { .. }
        | IssueKind::MixedMethodCall { .. }
        | IssueKind::MixedPropertyFetch { .. }
        | IssueKind::MixedPropertyAssignment { .. }
        | IssueKind::MixedArrayAccess
        | IssueKind::MixedArrayOffset => cfg.mixed_usage,
        _ => true,
    }
}

/// Returns true for issue kinds whose location was stored by the collector
/// (0-indexed columns). Body-analysis issues use `offset_to_line_col` which
/// is 1-indexed since mir 0.29; collector-stored locations were not changed.
fn uses_codebase_location(kind: &mir_issues::IssueKind) -> bool {
    use mir_issues::IssueKind;
    matches!(
        kind,
        IssueKind::CircularInheritance { .. }
            | IssueKind::InvalidExtendClass { .. }
            | IssueKind::UnimplementedAbstractMethod { .. }
            | IssueKind::UnimplementedInterfaceMethod { .. }
            | IssueKind::FinalMethodOverridden { .. }
            | IssueKind::OverriddenMethodAccess { .. }
            | IssueKind::MethodSignatureMismatch { .. }
            | IssueKind::InvalidTraitUse { .. }
    )
}

fn to_lsp_diagnostic(issue: mir_issues::Issue) -> Diagnostic {
    // mir 0.29+ uses 1-based lines everywhere; LSP uses 0-based.
    // Columns: body-analysis uses 1-indexed offset_to_line_col; collector-
    // stored locations (class/trait declarations) remain 0-indexed.
    let line = issue.location.line.saturating_sub(1);
    let (col_start, col_end) = if uses_codebase_location(&issue.kind) {
        (
            issue.location.col_start as u32,
            issue.location.col_end as u32,
        )
    } else {
        (
            issue.location.col_start.saturating_sub(1) as u32,
            issue.location.col_end.saturating_sub(1) as u32,
        )
    };
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
        use tower_lsp::lsp_types::NumberOrString;

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
