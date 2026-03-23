use std::sync::Arc;

use php_parser_rs::parser::ast::{
    classes::ClassMember, namespaces::NamespaceStatement, Statement,
};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::diagnostics::span_to_position;
use crate::util::word_at;

/// Find the definition of the symbol under `position`.
/// Searches the current document first, then `other_docs` for cross-file resolution.
pub fn goto_definition(
    uri: &Url,
    source: &str,
    ast: &[Statement],
    other_docs: &[(Url, Arc<Vec<Statement>>)],
    position: Position,
) -> Option<Location> {
    let word = word_at(source, position)?;

    if let Some(range) = scan_statements(ast, &word) {
        return Some(Location { uri: uri.clone(), range });
    }

    for (other_uri, other_ast) in other_docs {
        if let Some(range) = scan_statements(other_ast, &word) {
            return Some(Location { uri: other_uri.clone(), range });
        }
    }

    None
}

fn name_range(span: &php_parser_rs::lexer::token::Span, name: &str) -> Range {
    let start = span_to_position(span);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + name.len() as u32,
        },
    }
}

/// Search an AST for a declaration named `name`, returning its selection range.
/// Used by the PSR-4 fallback in the backend after resolving a class to a file.
pub fn find_declaration_range(stmts: &[Statement], name: &str) -> Option<Range> {
    scan_statements(stmts, name)
}

fn scan_statements(stmts: &[Statement], word: &str) -> Option<Range> {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) if f.name.value.to_string() == word => {
                return Some(name_range(&f.name.span, word));
            }
            Statement::Class(c) if c.name.value.to_string() == word => {
                return Some(name_range(&c.name.span, word));
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    match member {
                        ClassMember::ConcreteMethod(m) if m.name.value.to_string() == word => {
                            return Some(name_range(&m.name.span, word));
                        }
                        ClassMember::AbstractMethod(m) if m.name.value.to_string() == word => {
                            return Some(name_range(&m.name.span, word));
                        }
                        _ => {}
                    }
                }
            }
            Statement::Interface(i) if i.name.value.to_string() == word => {
                return Some(name_range(&i.name.span, word));
            }
            Statement::Trait(t) if t.name.value.to_string() == word => {
                return Some(name_range(&t.name.span, word));
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => scan_statements(&u.statements, word),
                    NamespaceStatement::Braced(b) => scan_statements(&b.body.statements, word),
                };
                if inner.is_some() {
                    return inner;
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn parse_ast(source: &str) -> Vec<Statement> {
        match php_parser_rs::parser::parse(source) {
            Ok(ast) => ast,
            Err(stack) => stack.partial,
        }
    }

    #[test]
    fn jumps_to_function_definition() {
        let src = "<?php\nfunction greet() {}";
        let ast = parse_ast(src);
        let result = goto_definition(&uri(), src, &ast, &[], pos(1, 10));
        assert!(result.is_some(), "expected a location");
        let loc = result.unwrap();
        assert_eq!(loc.range.start.line, 1);
        assert_eq!(loc.uri, uri());
    }

    #[test]
    fn jumps_to_class_definition() {
        let src = "<?php\nclass MyService {}";
        let ast = parse_ast(src);
        let result = goto_definition(&uri(), src, &ast, &[], pos(1, 8));
        assert!(result.is_some());
        let loc = result.unwrap();
        assert_eq!(loc.range.start.line, 1);
    }

    #[test]
    fn jumps_to_interface_definition() {
        let src = "<?php\ninterface Countable {}";
        let ast = parse_ast(src);
        let result = goto_definition(&uri(), src, &ast, &[], pos(1, 12));
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 1);
    }

    #[test]
    fn jumps_to_trait_definition() {
        let src = "<?php\ntrait Loggable {}";
        let ast = parse_ast(src);
        let result = goto_definition(&uri(), src, &ast, &[], pos(1, 8));
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 1);
    }

    #[test]
    fn jumps_to_class_method_definition() {
        let src = "<?php\nclass Calc { public function add() {} }";
        let ast = parse_ast(src);
        let result = goto_definition(&uri(), src, &ast, &[], pos(1, 32));
        assert!(result.is_some(), "expected location for method 'add'");
    }

    #[test]
    fn returns_none_for_unknown_word() {
        let src = "<?php\n$x = 1;";
        let ast = parse_ast(src);
        let result = goto_definition(&uri(), src, &ast, &[], pos(1, 1));
        assert!(result.is_none());
    }

    #[test]
    fn jumps_to_symbol_inside_namespace() {
        let src = "<?php\nnamespace App;\nfunction boot() {}";
        let ast = parse_ast(src);
        let result = goto_definition(&uri(), src, &ast, &[], pos(2, 10));
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 2);
    }

    #[test]
    fn finds_class_definition_in_other_document() {
        let current_src = "<?php\n$s = new MyService();";
        let current_ast = parse_ast(current_src);
        let other_src = "<?php\nclass MyService {}";
        let other_uri = Url::parse("file:///other.php").unwrap();
        let other_ast = Arc::new(parse_ast(other_src));

        let result = goto_definition(
            &uri(), current_src, &current_ast,
            &[(other_uri.clone(), other_ast)],
            pos(1, 13),
        );
        assert!(result.is_some(), "expected cross-file location");
        assert_eq!(result.unwrap().uri, other_uri);
    }

    #[test]
    fn finds_function_definition_in_other_document() {
        let current_src = "<?php\nhelperFn();";
        let current_ast = parse_ast(current_src);
        let other_src = "<?php\nfunction helperFn() {}";
        let other_uri = Url::parse("file:///helpers.php").unwrap();
        let other_ast = Arc::new(parse_ast(other_src));

        let result = goto_definition(
            &uri(), current_src, &current_ast,
            &[(other_uri.clone(), other_ast)],
            pos(1, 3),
        );
        assert!(result.is_some(), "expected cross-file location for helperFn");
        assert_eq!(result.unwrap().uri, other_uri);
    }

    #[test]
    fn current_file_takes_priority_over_other_docs() {
        let src = "<?php\nclass Foo {}";
        let ast = parse_ast(src);
        let other_src = "<?php\nclass Foo {}";
        let other_uri = Url::parse("file:///other.php").unwrap();
        let other_ast = Arc::new(parse_ast(other_src));

        let result = goto_definition(
            &uri(), src, &ast,
            &[(other_uri, other_ast)],
            pos(1, 8),
        );
        assert_eq!(result.unwrap().uri, uri(), "should prefer current file");
    }
}
