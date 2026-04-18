use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::ast::ParsedDoc;

/// Parse `source` and return the (owned) `ParsedDoc` plus any parse diagnostics.
pub fn parse_document(source: &str) -> (ParsedDoc, Vec<Diagnostic>) {
    let doc = ParsedDoc::parse(source.to_string());
    let sv = doc.view();
    let diagnostics = doc
        .errors
        .iter()
        .map(|e| {
            let span = e.span();
            let start = sv.position_of(span.start);
            let end = if span.end > span.start {
                sv.position_of(span.end)
            } else {
                // Zero-width span: advance by the UTF-16 width of the character
                // at the error position so the range is never a mid-surrogate
                // slice (characters outside the BMP take 2 UTF-16 code units).
                let ch_width = sv.source()[span.start as usize..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf16() as u32)
                    .unwrap_or(1);
                Position {
                    line: start.line,
                    character: start.character + ch_width,
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

    /// Probe: print every (start, end, zero_width) tuple for a wider set of
    /// error-inducing snippets to see if any zero-width span can be made to
    /// land *on* a non-BMP (surrogate-pair) character rather than at EOF.
    #[test]
    fn probe_zero_width_spans() {
        let cases: &[(&str, &str)] = &[
            ("class_no_name", "<?php\nclass {"),
            ("fn_no_name", "<?php\nfunction ("),
            ("assign_no_rhs", "<?php\n$x ="),
            ("bare_emoji", "<?php\n\u{1F600}"),
            ("emoji_class", "<?php\nclass \u{1F600} {"),
            // Try to force a zero-width span mid-file rather than at EOF.
            ("emoji_then_valid", "<?php\n\u{1F600}\nfunction f() {}"),
            ("emoji_in_string_ctx", "<?php\n$x = \u{1F600};"),
        ];
        for (label, src) in cases {
            let doc = crate::ast::ParsedDoc::parse(src.to_string());
            for e in &doc.errors {
                let span = e.span();
                let ch = src[span.start as usize..].chars().next();
                println!(
                    "{label}: span=({},{}) zero_width={} char={ch:?} src_len={}",
                    span.start,
                    span.end,
                    span.end == span.start,
                    src.len(),
                );
            }
        }
    }
}
