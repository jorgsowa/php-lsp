use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::ast::{ParsedDoc, offset_to_position};

/// Parse `source` and return the (owned) `ParsedDoc` plus any parse diagnostics.
pub fn parse_document(source: &str) -> (ParsedDoc, Vec<Diagnostic>) {
    let doc = ParsedDoc::parse(source.to_string());
    let diagnostics = doc
        .errors
        .iter()
        .map(|e| {
            let span = e.span();
            let start = offset_to_position(source, span.start);
            let end = if span.end > span.start {
                offset_to_position(source, span.end)
            } else {
                Position {
                    line: start.line,
                    character: start.character + 1,
                }
            };
            Diagnostic {
                range: Range { start, end },
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("php-lsp".to_string()),
                message: e.to_string(),
                ..Default::default()
            }
        })
        .collect();
    (doc, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_php_produces_no_diagnostics() {
        let (doc, diags) = parse_document("<?php\nfunction greet() {}");
        assert!(diags.is_empty());
        assert!(!doc.program().stmts.is_empty());
    }

    #[test]
    fn syntax_error_produces_diagnostic() {
        let (_, diags) = parse_document("<?php\nclass {");
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }
}
