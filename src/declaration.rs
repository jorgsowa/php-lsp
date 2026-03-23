/// `textDocument/declaration` — jump to the abstract or interface declaration of a symbol.
///
/// In PHP the distinction between declaration and definition matters for:
///   - Interface methods (declared but never given a body)
///   - Abstract class methods
///
/// For concrete symbols with no abstract counterpart this falls back to the same
/// result as go-to-definition so the request is never empty-handed.
use std::sync::Arc;

use php_parser_rs::parser::ast::{
    classes::ClassMember, namespaces::NamespaceStatement, Statement,
};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::diagnostics::span_to_position;
use crate::util::word_at;

/// Find the abstract or interface declaration of `word`.
/// Prefers abstract/interface declarations; falls back to any declaration.
pub fn goto_declaration(
    source: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
    position: Position,
) -> Option<Location> {
    let word = word_at(source, position)?;

    // First pass: look for an abstract or interface declaration
    for (uri, ast) in all_docs {
        if let Some(range) = find_abstract_declaration(ast, &word) {
            return Some(Location { uri: uri.clone(), range });
        }
    }

    // Second pass: any declaration (same as goto_definition)
    for (uri, ast) in all_docs {
        if let Some(range) = find_any_declaration(ast, &word) {
            return Some(Location { uri: uri.clone(), range });
        }
    }

    None
}

// ── Abstract / interface declarations ────────────────────────────────────────

fn find_abstract_declaration(stmts: &[Statement], word: &str) -> Option<Range> {
    for stmt in stmts {
        match stmt {
            Statement::Interface(i) => {
                // Interface methods are declarations without bodies
                for member in &i.body.members {
                    use php_parser_rs::parser::ast::interfaces::InterfaceMember;
                    if let InterfaceMember::Method(m) = member {
                        if m.name.value.to_string() == word {
                            return Some(name_range(&m.name.span, word));
                        }
                    }
                }
                // The interface name itself
                if i.name.value.to_string() == word {
                    return Some(name_range(&i.name.span, word));
                }
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    if let ClassMember::AbstractMethod(m) = member {
                        if m.name.value.to_string() == word {
                            return Some(name_range(&m.name.span, word));
                        }
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(r) = find_abstract_declaration(inner, word) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

// ── Fallback: any declaration (mirrors goto_definition) ──────────────────────

fn find_any_declaration(stmts: &[Statement], word: &str) -> Option<Range> {
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
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(r) = find_any_declaration(inner, word) {
                    return Some(r);
                }
            }
            _ => {}
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

    fn doc(path: &str, src: &str) -> (Url, Arc<Vec<Statement>>) {
        (uri(path), Arc::new(parse_ast(src)))
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn finds_interface_method_declaration() {
        let src = "<?php\ninterface Logger { public function log(string $msg): void; }\nclass FileLogger implements Logger { public function log(string $msg): void {} }";
        let docs = vec![doc("/a.php", src)];
        // cursor on "log" in the concrete implementation (line 2)
        let loc = goto_declaration(src, &docs, pos(2, 53));
        assert!(loc.is_some(), "expected a declaration location");
        // should jump to the interface declaration on line 1
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn finds_abstract_method_declaration() {
        let src = "<?php\nabstract class Base { abstract public function build(): void; }\nclass Impl extends Base { public function build(): void {} }";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(2, 42));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn falls_back_to_definition_for_concrete_function() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(2, 2));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn finds_interface_name_declaration() {
        let src = "<?php\ninterface Countable {}";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(1, 12));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn cross_file_interface_declaration() {
        let impl_src = "<?php\nclass Repo implements Countable { public function count(): int { return 0; } }";
        let iface_src = "<?php\ninterface Countable { public function count(): int; }";
        let iface_uri = uri("/iface.php");
        let docs = vec![
            doc("/impl.php", impl_src),
            (iface_uri.clone(), Arc::new(parse_ast(iface_src))),
        ];
        let loc = goto_declaration(impl_src, &docs, pos(1, 51));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().uri, iface_uri);
    }

    #[test]
    fn returns_none_for_unknown_word() {
        let src = "<?php\n$x = 1;";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(1, 1));
        assert!(loc.is_none());
    }
}
