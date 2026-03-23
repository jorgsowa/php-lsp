use std::sync::Arc;

use php_parser_rs::parser::ast::Statement;
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::diagnostics::span_to_position;
use crate::walk::{refs_in_stmts, refs_in_stmts_with_use};

/// Find all locations where `word` is referenced across the given documents.
/// If `include_declaration` is true, also includes the declaration site.
/// If `include_use_stmts` is true, also finds `use` statement spans.
pub fn find_references(
    word: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
    include_declaration: bool,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, false)
}

/// Like `find_references` but also includes `use` statement spans.
/// Used by rename so that `use Foo;` statements are also updated.
pub fn find_references_with_use(
    word: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
    include_declaration: bool,
) -> Vec<Location> {
    find_references_inner(word, all_docs, include_declaration, true)
}

fn find_references_inner(
    word: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
    include_declaration: bool,
    include_use: bool,
) -> Vec<Location> {
    let mut locations = Vec::new();

    for (uri, ast) in all_docs {
        let mut spans = Vec::new();
        if include_use {
            refs_in_stmts_with_use(ast, word, &mut spans);
        } else {
            refs_in_stmts(ast, word, &mut spans);
        }

        if !include_declaration {
            // Filter out declaration spans (the definition site)
            spans.retain(|span| !is_declaration_span(ast, word, span));
        }

        for span in spans {
            let start = span_to_position(&span);
            let end = Position {
                line: start.line,
                character: start.character + word.len() as u32,
            };
            locations.push(Location {
                uri: uri.clone(),
                range: Range { start, end },
            });
        }
    }

    locations
}

/// Returns true if this span is the declaration site (function/class/method name).
fn is_declaration_span(ast: &[Statement], word: &str, span: &php_parser_rs::lexer::token::Span) -> bool {
    use php_parser_rs::parser::ast::{classes::ClassMember, namespaces::NamespaceStatement};

    fn check(stmts: &[Statement], word: &str, span: &php_parser_rs::lexer::token::Span) -> bool {
        for stmt in stmts {
            match stmt {
                Statement::Function(f) if f.name.value.to_string() == word => {
                    if spans_equal(&f.name.span, span) { return true; }
                }
                Statement::Class(c) if c.name.value.to_string() == word => {
                    if spans_equal(&c.name.span, span) { return true; }
                }
                Statement::Class(c) => {
                    for member in &c.body.members {
                        match member {
                            ClassMember::ConcreteMethod(m) if m.name.value.to_string() == word => {
                                if spans_equal(&m.name.span, span) { return true; }
                            }
                            ClassMember::AbstractMethod(m) if m.name.value.to_string() == word => {
                                if spans_equal(&m.name.span, span) { return true; }
                            }
                            _ => {}
                        }
                    }
                }
                Statement::Interface(i) if i.name.value.to_string() == word => {
                    if spans_equal(&i.name.span, span) { return true; }
                }
                Statement::Trait(t) if t.name.value.to_string() == word => {
                    if spans_equal(&t.name.span, span) { return true; }
                }
                Statement::Namespace(ns) => {
                    let stmts = match ns {
                        NamespaceStatement::Unbraced(u) => &u.statements[..],
                        NamespaceStatement::Braced(b) => &b.body.statements[..],
                    };
                    if check(stmts, word, span) { return true; }
                }
                _ => {}
            }
        }
        false
    }

    check(ast, word, span)
}

fn spans_equal(a: &php_parser_rs::lexer::token::Span, b: &php_parser_rs::lexer::token::Span) -> bool {
    a.line == b.line && a.column == b.column
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ast(source: &str) -> Vec<Statement> {
        match php_parser_rs::parser::parse(source) {
            Ok(ast) => ast,
            Err(stack) => stack.partial,
        }
    }

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, source: &str) -> (Url, Arc<Vec<Statement>>) {
        (uri(path), Arc::new(parse_ast(source)))
    }

    #[test]
    fn finds_function_call_reference() {
        let src = "<?php\nfunction greet() {}\ngreet();\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false);
        assert_eq!(refs.len(), 2, "expected 2 call-site refs, got {:?}", refs);
    }

    #[test]
    fn include_declaration_adds_def_site() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let with_decl = find_references("greet", &docs, true);
        let without_decl = find_references("greet", &docs, false);
        assert!(with_decl.len() > without_decl.len(), "declaration should be included");
    }

    #[test]
    fn finds_new_expression_reference() {
        let src = "<?php\nclass Foo {}\n$x = new Foo();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("Foo", &docs, false);
        assert!(!refs.is_empty(), "expected reference to Foo in new expr");
    }

    #[test]
    fn finds_reference_in_nested_function_call() {
        let src = "<?php\nfunction greet() {}\necho(greet());";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("greet", &docs, false);
        assert!(!refs.is_empty(), "expected nested function call reference");
    }

    #[test]
    fn finds_references_across_multiple_docs() {
        let a = doc("/a.php", "<?php\nfunction helper() {}");
        let b = doc("/b.php", "<?php\nhelper();\nhelper();");
        let refs = find_references("helper", &[a, b], false);
        assert_eq!(refs.len(), 2, "expected 2 cross-file references");
        assert!(refs.iter().all(|r| r.uri.path().ends_with("/b.php")));
    }

    #[test]
    fn finds_method_call_reference() {
        let src = "<?php\nclass Calc { public function add() {} }\n$c = new Calc();\n$c->add();";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("add", &docs, false);
        assert!(!refs.is_empty(), "expected method call reference to 'add'");
    }

    #[test]
    fn finds_reference_inside_if_body() {
        let src = "<?php\nfunction check() {}\nif (true) { check(); }";
        let docs = vec![doc("/a.php", src)];
        let refs = find_references("check", &docs, false);
        assert!(!refs.is_empty(), "expected reference inside if body");
    }
}
