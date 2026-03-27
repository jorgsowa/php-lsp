/// Semantic diagnostics bridge.
///
/// Delegates all analysis to the `mir-php` crate and converts its `Diagnostic`
/// type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use std::sync::Arc;

use php_ast::{ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Position, Range, Url,
};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::docblock::{docblock_before, parse_docblock};

/// Run semantic checks on `doc` against `other_docs` and return LSP diagnostics.
pub fn semantic_diagnostics(
    uri: &Url,
    doc: &ParsedDoc,
    other_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<Diagnostic> {
    let source = doc.source();
    let stmts: &[php_ast::Stmt<'_, '_>] = doc.program().stmts.as_ref();

    // Build the workspace context: (source, stmts) for each document.
    let mut all: Vec<(&str, &[php_ast::Stmt<'_, '_>])> =
        Vec::with_capacity(1 + other_docs.len());
    all.push((source, stmts));
    for (_, d) in other_docs {
        all.push((d.source(), d.program().stmts.as_ref()));
    }

    mir_php::analyze(source, stmts, &all)
        .into_iter()
        .map(|d| to_lsp_diagnostic(d, uri))
        .collect()
}

/// Check for deprecated function/method calls and emit Warning diagnostics.
pub fn deprecated_call_diagnostics(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    collect_deprecated_calls(source, &doc.program().stmts, doc, other_docs, &mut diags);
    diags
}

fn collect_deprecated_calls(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    diags: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Expression(e) => {
                check_expr_for_deprecated(source, e, doc, other_docs, diags);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_deprecated_calls(source, inner, doc, other_docs, diags);
                }
            }
            StmtKind::Function(f) => {
                collect_deprecated_calls(source, &f.body, doc, other_docs, diags);
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let php_ast::ClassMemberKind::Method(m) = &member.kind {
                        if let Some(body) = &m.body {
                            collect_deprecated_calls(source, body, doc, other_docs, diags);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_expr_for_deprecated(
    source: &str,
    expr: &php_ast::Expr<'_, '_>,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    diags: &mut Vec<Diagnostic>,
) {
    if let ExprKind::Assign(a) = &expr.kind {
        check_expr_for_deprecated(source, a.value, doc, other_docs, diags);
        return;
    }
    if let ExprKind::FunctionCall(call) = &expr.kind {
        if let ExprKind::Identifier(name) = &call.name.kind {
            let func_name = name.as_ref();
            // Search all docs for this function's declaration
            let all_sources: Vec<(&str, &ParsedDoc)> = std::iter::once((source, doc))
                .chain(other_docs.iter().map(|d| (d.source(), d.as_ref())))
                .collect();
            for (src, d) in &all_sources {
                if let Some(span_start) = find_function_span(d, func_name) {
                    if let Some(raw) = docblock_before(src, span_start) {
                        let db = parse_docblock(&raw);
                        if db.is_deprecated() {
                            let start_pos = offset_to_position(source, call.name.span.start);
                            let end_pos = offset_to_position(source, call.name.span.end);
                            let msg = match &db.deprecated {
                                Some(m) if !m.is_empty() => format!("Deprecated: {} — {}", func_name, m),
                                _ => format!("Deprecated: {}", func_name),
                            };
                            diags.push(Diagnostic {
                                range: Range {
                                    start: Position { line: start_pos.line, character: start_pos.character },
                                    end: Position { line: end_pos.line, character: end_pos.character },
                                },
                                severity: Some(DiagnosticSeverity::WARNING),
                                source: Some("php-lsp".to_string()),
                                message: msg,
                                ..Default::default()
                            });
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn find_function_span(doc: &ParsedDoc, func_name: &str) -> Option<u32> {
    find_function_span_in_stmts(&doc.program().stmts, func_name)
}

fn find_function_span_in_stmts(stmts: &[Stmt<'_, '_>], func_name: &str) -> Option<u32> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == func_name => {
                return Some(stmt.span.start);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(s) = find_function_span_in_stmts(inner, func_name) {
                        return Some(s);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Check for duplicate class/function/interface/trait/enum declarations.
pub fn duplicate_declaration_diagnostics(source: &str, doc: &ParsedDoc) -> Vec<Diagnostic> {
    let mut seen: std::collections::HashMap<String, ()> = std::collections::HashMap::new();
    let mut diags = Vec::new();
    collect_duplicate_decls(source, &doc.program().stmts, &mut seen, &mut diags);
    diags
}

fn collect_duplicate_decls(
    source: &str,
    stmts: &[php_ast::Stmt<'_, '_>],
    seen: &mut std::collections::HashMap<String, ()>,
    diags: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        let name_and_span: Option<(&str, u32)> = match &stmt.kind {
            StmtKind::Class(c) => c.name.map(|n| (n, stmt.span.start)),
            StmtKind::Interface(i) => Some((i.name, stmt.span.start)),
            StmtKind::Trait(t) => Some((t.name, stmt.span.start)),
            StmtKind::Enum(e) => Some((e.name, stmt.span.start)),
            StmtKind::Function(f) => Some((f.name, stmt.span.start)),
            StmtKind::Namespace(ns) => {
                if let php_ast::NamespaceBody::Braced(inner) = &ns.body {
                    collect_duplicate_decls(source, inner, seen, diags);
                }
                None
            }
            _ => None,
        };
        if let Some((name, span_start)) = name_and_span {
            if seen.insert(name.to_string(), ()).is_some() {
                let pos = crate::ast::offset_to_position(source, span_start);
                diags.push(Diagnostic {
                    range: Range { start: pos, end: pos },
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!("Duplicate declaration: `{name}` is already defined in this file"),
                    source: Some("php-lsp".to_string()),
                    ..Default::default()
                });
            }
        }
    }
}

fn to_lsp_diagnostic(d: mir_php::Diagnostic, uri: &Url) -> Diagnostic {
    let related_information = if d.related.is_empty() {
        None
    } else {
        Some(
            d.related
                .into_iter()
                .map(|(sl, sc, el, ec, msg)| DiagnosticRelatedInformation {
                    location: Location {
                        uri: uri.clone(),
                        range: Range {
                            start: Position { line: sl, character: sc },
                            end: Position { line: el, character: ec },
                        },
                    },
                    message: msg,
                })
                .collect(),
        )
    };
    Diagnostic {
        range: Range {
            start: Position { line: d.start_line, character: d.start_char },
            end: Position { line: d.end_line, character: d.end_char },
        },
        severity: Some(match d.severity {
            mir_php::Severity::Error => DiagnosticSeverity::ERROR,
            mir_php::Severity::Warning => DiagnosticSeverity::WARNING,
            mir_php::Severity::Information => DiagnosticSeverity::INFORMATION,
            mir_php::Severity::Hint => DiagnosticSeverity::HINT,
        }),
        source: Some("php-lsp".to_string()),
        message: d.message,
        related_information,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecated_function_call_emits_warning() {
        let src = "<?php\n/** @deprecated Use newFunc() instead */\nfunction oldFunc() {}\n\noldFunc();";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = deprecated_call_diagnostics(src, &doc, &[]);
        assert!(!diags.is_empty(), "expected a deprecated warning diagnostic");
        let d = &diags[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
        assert!(d.message.contains("oldFunc"), "message should mention the function name");
    }

    #[test]
    fn duplicate_class_emits_warning() {
        let src = "<?php\nclass Foo {}\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc);
        assert!(!diags.is_empty(), "should warn on duplicate class");
        assert!(diags[0].message.contains("Foo"));
    }

    #[test]
    fn no_duplicate_for_unique_declarations() {
        let src = "<?php\nclass Foo {}\nclass Bar {}";
        let doc = ParsedDoc::parse(src.to_string());
        let diags = duplicate_declaration_diagnostics(src, &doc);
        assert!(diags.is_empty());
    }
}
