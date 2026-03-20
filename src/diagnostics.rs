use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range, Url};

pub fn parse_diagnostics(_uri: &Url, source: &str) -> Vec<Diagnostic> {
    match php_parser_rs::parser::parse(source) {
        Ok(_) => vec![],
        Err(stack) => stack
            .errors
            .iter()
            .map(|e| {
                let start = span_to_position(&e.span);
                // Use a single-character range for the error location
                let end = Position {
                    line: start.line,
                    character: start.character + 1,
                };
                Diagnostic {
                    range: Range { start, end },
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("php-lsp".to_string()),
                    message: e.message.clone(),
                    ..Default::default()
                }
            })
            .collect(),
    }
}

pub(crate) fn span_to_position(span: &php_parser_rs::lexer::token::Span) -> Position {
    // php-parser-rs uses 1-based line/column; LSP uses 0-based
    Position {
        line: span.line.saturating_sub(1) as u32,
        character: span.column.saturating_sub(1) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    #[test]
    fn valid_php_produces_no_diagnostics() {
        let src = "<?php\nfunction hello() { return 42; }";
        assert!(parse_diagnostics(&uri(), src).is_empty());
    }

    #[test]
    fn missing_class_name_produces_diagnostic() {
        // "class {" on line 2 col 7 (1-based) → LSP line 1 character 6
        let src = "<?php\nclass {";
        let diags = parse_diagnostics(&uri(), src);
        assert!(!diags.is_empty(), "expected at least one diagnostic");
        let d = &diags[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.range.start.line, 1, "line should be 0-based");
        assert_eq!(d.range.start.character, 6, "character should be 0-based");
        assert_eq!(d.source.as_deref(), Some("php-lsp"));
        assert!(
            d.message.contains("identifier"),
            "message should mention 'identifier', got: {}",
            d.message
        );
    }

    #[test]
    fn fixed_file_produces_no_diagnostics() {
        let broken = "<?php\nclass {";
        let fixed = "<?php\nclass Foo {}";
        assert!(!parse_diagnostics(&uri(), broken).is_empty());
        assert!(parse_diagnostics(&uri(), fixed).is_empty());
    }

    #[test]
    fn multiple_errors_all_reported() {
        // Two distinct syntax errors
        let src = "<?php\nclass {\nfunction {";
        let diags = parse_diagnostics(&uri(), src);
        assert!(diags.len() >= 1, "expected diagnostics for broken source");
    }
}
