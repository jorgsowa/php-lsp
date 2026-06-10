/// Semantic diagnostics bridge.
///
/// Delegates all analysis to the `mir-analyzer` crate and converts its `Issue`
/// type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use php_ast::StmtKind;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::analysis::diagnostics::PHP_LSP_SOURCE;
use crate::ast::{ParsedDoc, SourceView};
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
        IssueKind::DuplicateClass { .. } => cfg.duplicate_declarations,
        // mir 0.22 unused-symbol warnings. Off by default; opt in via
        // `diagnostics.unusedSymbols` in initializationOptions.
        IssueKind::UnusedVariable { .. }
        | IssueKind::UnusedParam { .. }
        | IssueKind::UnusedMethod { .. }
        | IssueKind::UnusedProperty { .. }
        | IssueKind::UnusedFunction { .. } => cfg.unused_symbols,
        _ => true,
    }
}

/// Check for duplicate class/function/interface/trait/enum declarations.
pub fn duplicate_declaration_diagnostics(
    _source: &str,
    doc: &ParsedDoc,
    cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    if !cfg.enabled || !cfg.duplicate_declarations {
        return vec![];
    }
    let sv = doc.view();
    let mut seen: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
    let mut diags = Vec::new();
    collect_duplicate_decls(sv, &doc.program().stmts, "", &mut seen, &mut diags);
    diags
}

fn collect_duplicate_decls(
    sv: SourceView<'_>,
    stmts: &[php_ast::Stmt<'_, '_>],
    current_ns: &str,
    seen: &mut std::collections::HashMap<String, ()>,
    diags: &mut Vec<Diagnostic>,
) {
    // Track the active namespace for unbraced `namespace Foo;` declarations.
    let mut active_ns = current_ns.to_string();

    for stmt in stmts {
        let name_and_span: Option<(String, u32)> = match &stmt.kind {
            StmtKind::Interface(i) => Some((i.name.to_string(), stmt.span.start)),
            StmtKind::Trait(t) => Some((t.name.to_string(), stmt.span.start)),
            StmtKind::Enum(e) => Some((e.name.to_string(), stmt.span.start)),
            StmtKind::Function(f) => Some((f.name.to_string(), stmt.span.start)),
            StmtKind::Namespace(ns) => {
                let ns_name = ns
                    .name
                    .as_ref()
                    .map(|n| n.to_string_repr().to_string())
                    .unwrap_or_default();
                match &ns.body {
                    php_ast::NamespaceBody::Braced(inner) => {
                        let child_ns = if current_ns.is_empty() {
                            ns_name
                        } else {
                            format!("{}\\{}", current_ns, ns_name)
                        };
                        collect_duplicate_decls(sv, &inner.stmts, &child_ns, seen, diags);
                    }
                    php_ast::NamespaceBody::Simple => {
                        // Unbraced namespace: subsequent siblings belong to this namespace.
                        active_ns = if current_ns.is_empty() {
                            ns_name
                        } else {
                            format!("{}\\{}", current_ns, ns_name)
                        };
                    }
                }
                None
            }
            _ => None,
        };
        if let Some((name, span_start)) = name_and_span {
            let key = if active_ns.is_empty() {
                name.clone()
            } else {
                format!("{}\\{}", active_ns, name)
            };
            if seen.insert(key, ()).is_some() {
                // Find the byte offset of the actual name by searching forward from span_start.
                // The span_start points to keywords like "class", "function", etc.,
                // so we need to find where the identifier name appears.
                let name_byte_offset = find_name_offset(&sv.source()[span_start as usize..], &name)
                    .map(|off| span_start + off as u32)
                    .unwrap_or(span_start);

                let start_pos = sv.position_of(name_byte_offset);
                // Calculate end position by converting UTF-8 character length to UTF-16 code units
                let name_utf16_len = name.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
                let end_pos = Position {
                    line: start_pos.line,
                    character: start_pos.character + name_utf16_len,
                };
                diags.push(Diagnostic {
                    range: Range {
                        start: start_pos,
                        end: end_pos,
                    },
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "Duplicate declaration: `{name}` is already defined in this file"
                    ),
                    source: Some(PHP_LSP_SOURCE.to_string()),
                    ..Default::default()
                });
            }
        }
    }
}

/// Find the byte offset of an identifier name within a sv.source() slice.
/// Searches for word boundary matches (not substring matches).
fn find_name_offset(source: &str, name: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    for i in 0..source.len() {
        if source[i..].starts_with(name) {
            // Check word boundary before
            let before_ok = i == 0 || !is_identifier_char(bytes[i - 1] as char);
            // Check word boundary after
            let after_idx = i + name.len();
            let after_ok =
                after_idx >= source.len() || !is_identifier_char(bytes[after_idx] as char);
            if before_ok && after_ok {
                return Some(i);
            }
        }
    }
    None
}

/// Check if a character is valid in a PHP identifier.
fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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
    fn duplicate_interface_declaration() {
        // Same interface defined twice in same file — should produce exactly one error.
        let src = "<?php\ninterface Logger {}\ninterface Logger {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(
            diags.len(),
            1,
            "expected exactly 1 duplicate-declaration diagnostic, got: {:?}",
            diags
        );
        assert!(
            diags[0].message.contains("Logger"),
            "diagnostic message should mention 'Logger'"
        );
        assert_eq!(
            diags[0].severity,
            Some(DiagnosticSeverity::WARNING),
            "duplicate declaration should be a warning"
        );
    }

    #[test]
    fn duplicate_trait_declaration() {
        // Same trait defined twice in same file — should produce exactly one error.
        let src = "<?php\ntrait Serializable {}\ntrait Serializable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(
            diags.len(),
            1,
            "expected exactly 1 duplicate-declaration diagnostic, got: {:?}",
            diags
        );
        assert!(
            diags[0].message.contains("Serializable"),
            "diagnostic message should mention 'Serializable'"
        );
        assert_eq!(
            diags[0].severity,
            Some(DiagnosticSeverity::WARNING),
            "duplicate trait declaration should be a warning"
        );
    }

    #[test]
    fn duplicate_diagnostic_has_warning_severity() {
        // Duplicate declarations are reported as WARNING by our implementation.
        // (Note: `duplicate_declaration_diagnostics` emits DiagnosticSeverity::WARNING.)
        let src = "<?php\nfunction doWork() {}\nfunction doWork() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");
        assert_eq!(
            diags[0].severity,
            Some(DiagnosticSeverity::WARNING),
            "duplicate declaration diagnostic should have WARNING severity"
        );
    }

    #[test]
    fn duplicate_function_declaration_range_spans_name() {
        // Function duplicate should also span the full function name.
        let src = "<?php\nfunction doWork() {}\nfunction doWork() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");

        let d = &diags[0];
        let range_len = d.range.end.character - d.range.start.character;
        let expected_len = "doWork".chars().map(|c| c.len_utf16() as u32).sum::<u32>();
        assert_eq!(
            range_len, expected_len,
            "range length {} should match 'doWork' length {}",
            range_len, expected_len
        );

        // Verify the range points to "doWork", not "function"
        // "doWork" appears at character position 9 on line 2: "function doWork() {}"
        //                                              0123456789...
        assert_eq!(
            d.range.start.character, 9,
            "range should start at 'd' in 'doWork'"
        );
        assert_eq!(
            d.range.end.character, 15,
            "range should end after 'k' in 'doWork'"
        );
    }

    #[test]
    fn duplicate_interface_range_spans_name() {
        // Interface duplicate should span the full interface name.
        let src = "<?php\ninterface Logger {}\ninterface Logger {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");

        let d = &diags[0];
        let range_len = d.range.end.character - d.range.start.character;
        let expected_len = "Logger".chars().map(|c| c.len_utf16() as u32).sum::<u32>();
        assert_eq!(
            range_len, expected_len,
            "range length {} should match 'Logger' length {}",
            range_len, expected_len
        );

        // Verify the range points to "Logger", not "interface"
        // "Logger" appears at character position 10 on line 2: "interface Logger {}"
        //                                               01234567890...
        assert_eq!(
            d.range.start.character, 10,
            "range should start at 'L' in 'Logger'"
        );
        assert_eq!(
            d.range.end.character, 16,
            "range should end after 'r' in 'Logger'"
        );
    }

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
