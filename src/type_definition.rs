/// `textDocument/typeDefinition` — jump to the class declaration of the type
/// of the symbol under the cursor.
///
/// Works for variables assigned via `$var = new ClassName()` (leverages `TypeMap`)
/// and for function parameters with a declared type hint.
use std::sync::Arc;

use php_parser_rs::parser::ast::{namespaces::NamespaceStatement, Statement};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::diagnostics::span_to_position;
use crate::type_map::TypeMap;
use crate::util::word_at;

/// Given the cursor position, resolve the type of the symbol and return the
/// location of that type's class/interface declaration.
pub fn goto_type_definition(
    source: &str,
    ast: &[Statement],
    all_docs: &[(Url, Arc<Vec<Statement>>)],
    position: Position,
) -> Option<Location> {
    let word = word_at(source, position)?;

    // Resolve variable → class name via type map
    let type_map = TypeMap::from_stmts(ast);
    let class_name = if word.starts_with('$') {
        type_map.get(&word)?.to_string()
    } else {
        // Also check function parameter types
        param_type_for(ast, &word)?
    };

    // Find the class/interface declaration across all docs
    for (uri, doc_ast) in all_docs {
        if let Some(range) = find_class_range(doc_ast, &class_name) {
            return Some(Location { uri: uri.clone(), range });
        }
    }
    None
}

/// Look up the declared type hint for a parameter named `word` in any function/method.
fn param_type_for(stmts: &[Statement], word: &str) -> Option<String> {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) => {
                for p in f.parameters.parameters.iter() {
                    if p.name.name.to_string() == word {
                        if let Some(t) = &p.data_type {
                            return Some(t.to_string());
                        }
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(t) = param_type_for(inner, word) {
                    return Some(t);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the range of the class or interface declaration named `name`.
fn find_class_range(stmts: &[Statement], name: &str) -> Option<Range> {
    for stmt in stmts {
        match stmt {
            Statement::Class(c) if c.name.value.to_string() == name => {
                let start = span_to_position(&c.name.span);
                return Some(Range {
                    start,
                    end: Position {
                        line: start.line,
                        character: start.character + name.len() as u32,
                    },
                });
            }
            Statement::Interface(i) if i.name.value.to_string() == name => {
                let start = span_to_position(&i.name.span);
                return Some(Range {
                    start,
                    end: Position {
                        line: start.line,
                        character: start.character + name.len() as u32,
                    },
                });
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(r) = find_class_range(inner, name) {
                    return Some(r);
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

    fn parse_ast(source: &str) -> Vec<Statement> {
        match php_parser_rs::parser::parse(source) {
            Ok(ast) => ast,
            Err(stack) => stack.partial,
        }
    }

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn resolves_variable_type_to_class() {
        let src = "<?php\nclass Foo {}\n$obj = new Foo();\n$obj->bar();";
        let ast = parse_ast(src);
        let docs = vec![(uri("/a.php"), Arc::new(ast.clone()))];
        let loc = goto_type_definition(src, &ast, &docs, pos(3, 2));
        assert!(loc.is_some(), "expected type definition for $obj");
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn cross_file_type_definition() {
        let src = "<?php\n$obj = new Mailer();\n$obj->send();";
        let ast = parse_ast(src);
        let other_src = "<?php\nclass Mailer {}";
        let other_uri = uri("/mailer.php");
        let docs = vec![
            (uri("/a.php"), Arc::new(ast.clone())),
            (other_uri.clone(), Arc::new(parse_ast(other_src))),
        ];
        let loc = goto_type_definition(src, &ast, &docs, pos(2, 2));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().uri, other_uri);
    }

    #[test]
    fn unknown_variable_returns_none() {
        let src = "<?php\n$unknown->foo();";
        let ast = parse_ast(src);
        let docs = vec![(uri("/a.php"), Arc::new(ast.clone()))];
        let loc = goto_type_definition(src, &ast, &docs, pos(1, 2));
        assert!(loc.is_none());
    }

    #[test]
    fn resolves_interface_type() {
        let src = "<?php\ninterface Countable {}\n$obj = new MyList();\nclass MyList implements Countable {}";
        let ast = parse_ast(src);
        let docs = vec![(uri("/a.php"), Arc::new(ast.clone()))];
        // cursor on "$obj" — type is MyList
        let loc = goto_type_definition(src, &ast, &docs, pos(2, 2));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().range.start.line, 3);
    }

    #[test]
    fn returns_none_for_non_variable_without_type() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let ast = parse_ast(src);
        let docs = vec![(uri("/a.php"), Arc::new(ast.clone()))];
        let loc = goto_type_definition(src, &ast, &docs, pos(2, 2));
        assert!(loc.is_none());
    }
}
