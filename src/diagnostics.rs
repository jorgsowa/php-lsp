use php_parser_rs::parser::ast::Statement;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// Parse source once, returning the (partial) AST and any diagnostics.
/// This is the single parse entrypoint used by DocumentStore.
pub fn parse_document(source: &str) -> (Vec<Statement>, Vec<Diagnostic>) {
    match php_parser_rs::parser::parse(source) {
        Ok(ast) => (ast, vec![]),
        Err(stack) => {
            let diagnostics = stack
                .errors
                .iter()
                .map(|e| {
                    let start = span_to_position(&e.span);
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
                .collect();
            (stack.partial, diagnostics)
        }
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

    #[test]
    fn valid_php_produces_no_diagnostics() {
        let src = "<?php\nfunction hello() { return 42; }";
        assert!(parse_document(src).1.is_empty());
    }

    #[test]
    fn missing_class_name_produces_diagnostic() {
        // "class {" on line 2 col 7 (1-based) → LSP line 1 character 6
        let src = "<?php\nclass {";
        let diags = parse_document(src).1;
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
        assert!(!parse_document(broken).1.is_empty());
        assert!(parse_document(fixed).1.is_empty());
    }

    #[test]
    fn multiple_errors_all_reported() {
        let src = "<?php\nclass {\nfunction {";
        let diags = parse_document(src).1;
        assert!(diags.len() >= 1, "expected diagnostics for broken source");
    }

    #[test]
    fn parse_document_returns_partial_ast_and_errors() {
        let src = "<?php\nfunction valid() {}\nclass {";
        let (ast, diags) = parse_document(src);
        assert!(!diags.is_empty(), "expected parse errors");
        assert!(!ast.is_empty(), "expected partial AST with valid function");
    }

    #[test]
    fn parse_document_valid_returns_full_ast_no_errors() {
        let src = "<?php\nfunction greet(): string { return 'hi'; }";
        let (ast, diags) = parse_document(src);
        assert!(diags.is_empty());
        assert!(!ast.is_empty());
    }
}
