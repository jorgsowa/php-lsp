/// Semantic diagnostics bridge.
///
/// Delegates all analysis to the `mir-php` crate and converts its `Diagnostic`
/// type into the `tower-lsp` `Diagnostic` type expected by the LSP backend.
use std::sync::Arc;

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Position, Range, Url,
};

use crate::ast::ParsedDoc;

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
