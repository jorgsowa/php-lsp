/// Semantic diagnostics bridge.
///
/// Delegates all analysis to the `mir-analyzer` crate and converts its `Issue`
/// type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use php_ast::StmtKind;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Url};

use crate::ast::{ParsedDoc, SourceView};
use crate::backend::DiagnosticsConfig;

/// Run semantic checks on `doc` using the backend's persistent codebase.
/// The codebase is updated incrementally: the current file's definitions are
/// evicted and re-collected, then `finalize()` rebuilds inheritance tables.
///
/// `php_version` is a version string like `"8.1"` sourced from `LspConfig`.
/// Parsed to `mir_analyzer::PhpVersion` and forwarded to `StatementsAnalyzer`.
///
/// Legacy mutating path — runs `remove_file_definitions` + collect + finalize
/// on the codebase. Kept for benchmarks (`benches/semantic.rs`) and as the
/// reference implementation while Phase D wraps Pass-2 in salsa. Not used by
/// the LSP handlers anymore (they use `semantic_diagnostics_no_rebuild`
/// against the salsa-built codebase).
pub fn semantic_diagnostics(
    uri: &Url,
    doc: &ParsedDoc,
    codebase: &mir_codebase::Codebase,
    cfg: &DiagnosticsConfig,
    php_version: Option<&str>,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }

    let file: std::sync::Arc<str> = std::sync::Arc::from(uri.as_str());

    // Incremental update: evict stale definitions for this file, re-collect,
    // and rebuild inheritance tables.
    codebase.remove_file_definitions(&file);
    let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());
    let collector_issues = {
        let _span = tracing::debug_span!("collect_definitions", file = %uri).entered();
        let collector = mir_analyzer::collector::DefinitionCollector::new(
            codebase,
            file.clone(),
            doc.source(),
            &source_map,
        );
        collector.collect(doc.program())
    };
    {
        let _span = tracing::debug_span!("codebase_finalize", file = %uri).entered();
        codebase.finalize();
    }

    // Pass 2: analyse function/method bodies in the current document.
    let ver = php_version
        .and_then(|s| s.parse::<mir_analyzer::PhpVersion>().ok())
        .unwrap_or(mir_analyzer::PhpVersion::LATEST);
    let mut issue_buffer = mir_issues::IssueBuffer::new();
    let mut symbols = Vec::new();
    let mut analyzer = mir_analyzer::stmt::StatementsAnalyzer::new(
        codebase,
        file.clone(),
        doc.source(),
        &source_map,
        &mut issue_buffer,
        &mut symbols,
        ver,
        false,
    );
    let mut ctx = mir_analyzer::context::Context::new();
    {
        let _span = tracing::debug_span!("analyze_stmts", file = %uri).entered();
        analyzer.analyze_stmts(&doc.program().stmts, &mut ctx);
    }

    collector_issues
        .into_iter()
        .chain(issue_buffer.into_issues())
        .filter(|i| !i.suppressed)
        .filter(|i| issue_passes_filter(i, cfg))
        .map(|i| to_lsp_diagnostic(i, uri))
        .collect()
}

/// Run semantic body analysis on `doc` assuming the codebase is already
/// finalized (all definitions collected, `finalize()` already called).
///
/// Unlike [`semantic_diagnostics`], this function does **not** mutate the
/// codebase — it skips the `remove_file_definitions` / re-collect / `finalize`
/// cycle. Intended for workspace diagnostic batch passes where the codebase is
/// built once upfront and `finalize()` is called a single time before the loop.
///
/// Phase I: LSP handlers now read issues through the salsa `semantic_issues`
/// query + `issues_to_diagnostics`. This function is retained for
/// `benches/semantic.rs` as a single-call reference implementation.
pub fn semantic_diagnostics_no_rebuild(
    uri: &Url,
    doc: &ParsedDoc,
    codebase: &mir_codebase::Codebase,
    cfg: &DiagnosticsConfig,
    php_version: Option<&str>,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }

    let file: std::sync::Arc<str> = std::sync::Arc::from(uri.as_str());
    let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());

    // Pass 2 only: analyse function/method bodies.
    // The codebase is already finalized — skip remove/re-collect/finalize so
    // that inheritance tables are not torn down and rebuilt for every file.
    let ver = php_version
        .and_then(|s| s.parse::<mir_analyzer::PhpVersion>().ok())
        .unwrap_or(mir_analyzer::PhpVersion::LATEST);
    let mut issue_buffer = mir_issues::IssueBuffer::new();
    let mut symbols = Vec::new();
    let mut analyzer = mir_analyzer::stmt::StatementsAnalyzer::new(
        codebase,
        file,
        doc.source(),
        &source_map,
        &mut issue_buffer,
        &mut symbols,
        ver,
        false,
    );
    let mut ctx = mir_analyzer::context::Context::new();
    analyzer.analyze_stmts(&doc.program().stmts, &mut ctx);

    issue_buffer
        .into_issues()
        .into_iter()
        .filter(|i| !i.suppressed)
        .filter(|i| issue_passes_filter(i, cfg))
        .map(|i| to_lsp_diagnostic(i, uri))
        .collect()
}

/// Convert pre-computed raw issues (from `db::semantic::semantic_issues`) into
/// LSP diagnostics, applying the user's `DiagnosticsConfig` filter. Keeping
/// filter + conversion outside the salsa query preserves memoization across
/// config toggles (the user flipping a category must not rerun the analyzer).
pub fn issues_to_diagnostics(
    issues: &[mir_issues::Issue],
    uri: &Url,
    cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    if !cfg.enabled {
        return vec![];
    }
    issues
        .iter()
        .filter(|i| issue_passes_filter(i, cfg))
        .cloned()
        .map(|i| to_lsp_diagnostic(i, uri))
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
        IssueKind::UndefinedClass { .. } => cfg.undefined_classes,
        // InvalidArgument covers both arity errors and type mismatches in mir-analyzer;
        // show it if either toggle is on.
        IssueKind::InvalidArgument { .. } => cfg.arity_errors || cfg.type_errors,
        IssueKind::InvalidReturnType { .. }
        | IssueKind::NullMethodCall { .. }
        | IssueKind::NullPropertyFetch { .. }
        | IssueKind::NullableReturnStatement { .. }
        | IssueKind::InvalidPropertyAssignment { .. }
        | IssueKind::InvalidOperand { .. } => cfg.type_errors,
        IssueKind::DeprecatedCall { .. }
        | IssueKind::DeprecatedMethodCall { .. }
        | IssueKind::DeprecatedMethod { .. }
        | IssueKind::DeprecatedClass { .. } => cfg.deprecated_calls,
        IssueKind::InvalidNamedArgument { .. } => cfg.arity_errors,
        IssueKind::CircularInheritance { .. } => true,
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
        let name_and_span: Option<(&str, u32)> = match &stmt.kind {
            StmtKind::Class(c) => c.name.map(|n| (n, stmt.span.start)),
            StmtKind::Interface(i) => Some((i.name, stmt.span.start)),
            StmtKind::Trait(t) => Some((t.name, stmt.span.start)),
            StmtKind::Enum(e) => Some((e.name, stmt.span.start)),
            StmtKind::Function(f) => Some((f.name, stmt.span.start)),
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
                        collect_duplicate_decls(sv, inner, &child_ns, seen, diags);
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
                name.to_string()
            } else {
                format!("{}\\{}", active_ns, name)
            };
            if seen.insert(key, ()).is_some() {
                // Find the byte offset of the actual name by searching forward from span_start.
                // The span_start points to keywords like "class", "function", etc.,
                // so we need to find where the identifier name appears.
                let name_byte_offset = find_name_offset(&sv.source()[span_start as usize..], name)
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
                    source: Some("php-lsp".to_string()),
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

fn to_lsp_diagnostic(issue: mir_issues::Issue, _uri: &Url) -> Diagnostic {
    // mir-analyzer uses 1-based line numbers; LSP uses 0-based.
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
        source: Some("php-lsp".to_string()),
        message: issue.kind.message(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_class_emits_warning() {
        let src = "<?php\nclass Foo {}\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(
            diags.len(),
            1,
            "expected exactly 1 duplicate warning, got: {:?}",
            diags
        );
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(
            diags[0].message.contains("Foo"),
            "message should mention 'Foo'"
        );
    }

    #[test]
    fn no_duplicate_for_unique_declarations() {
        let src = "<?php\nclass Foo {}\nclass Bar {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert!(diags.is_empty());
    }

    #[test]
    fn namespace_scoped_duplicate_not_flagged() {
        // Two classes named `Foo` in different namespaces — should produce zero diagnostics.
        let src = "<?php\nnamespace App\\A {\nclass Foo {}\n}\nnamespace App\\B {\nclass Foo {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert!(
            diags.is_empty(),
            "classes with same name in different namespaces should not be flagged, got: {:?}",
            diags
        );
    }

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
    fn unbraced_namespace_classes_with_same_name_not_flagged() {
        // Two classes named `Foo` in different unbraced namespaces — should not be a duplicate.
        let src = "<?php\nnamespace App\\A;\nclass Foo {}\nnamespace App\\B;\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert!(
            diags.is_empty(),
            "classes with same name in different unbraced namespaces should not be flagged, got: {:?}",
            diags
        );
    }

    #[test]
    fn unbraced_namespace_duplicate_in_same_namespace_is_flagged() {
        // Two classes named `Foo` in the same unbraced namespace — should produce one warning.
        let src = "<?php\nnamespace App;\nclass Foo {}\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(
            diags.len(),
            1,
            "expected 1 duplicate-declaration diagnostic, got: {:?}",
            diags
        );
        assert!(diags[0].message.contains("Foo"));
    }

    #[test]
    fn duplicate_declaration_range_spans_full_name() {
        // Duplicate declaration diagnostic range should span the entire name, not just first character.
        let src = "<?php\nclass Foo {}\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");

        let d = &diags[0];
        let range_len = d.range.end.character - d.range.start.character;
        let expected_len = "Foo".chars().map(|c| c.len_utf16() as u32).sum::<u32>();
        assert_eq!(
            range_len, expected_len,
            "range length {} should match 'Foo' length {}",
            range_len, expected_len
        );

        // Verify the range actually points to "Foo", not "class"
        // "Foo" appears at character position 6 on line 2: "class Foo {}"
        //                                          012345678...
        assert_eq!(
            d.range.start.character, 6,
            "range should start at 'F' in 'Foo'"
        );
        assert_eq!(
            d.range.end.character, 9,
            "range should end after 'o' in 'Foo'"
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
    fn duplicate_declaration_range_on_correct_line() {
        // Diagnostic range should be on the correct line.
        let src = "<?php\nclass Foo {}\n\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc, &DiagnosticsConfig::all_enabled());
        assert_eq!(diags.len(), 1, "expected exactly 1 duplicate diagnostic");

        let d = &diags[0];
        // The second "class Foo" is on line 3 (0-indexed: line 3)
        assert_eq!(
            d.range.start.line, 3,
            "duplicate should be reported on line 3 (0-indexed)"
        );
        assert_eq!(
            d.range.end.line, 3,
            "range end should be on same line as start"
        );
    }

    #[test]
    fn to_lsp_diagnostic_sets_code_to_issue_kind_name() {
        use mir_issues::{Issue, IssueKind, Location};
        use std::sync::Arc;
        use tower_lsp::lsp_types::{NumberOrString, Url};

        let uri = Url::parse("file:///test.php").unwrap();
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
        let diag = to_lsp_diagnostic(issue, &uri);
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
