/// `textDocument/implementation` — find all classes that implement an interface
/// or extend a class with the given name.
use std::sync::Arc;

use php_parser_rs::parser::ast::{namespaces::NamespaceStatement, Statement};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::diagnostics::span_to_position;
use crate::util::word_at;

/// Return all `Location`s where a class declares `extends Name` or
/// `implements Name`.
pub fn find_implementations(
    word: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Vec<Location> {
    let mut locations = Vec::new();
    for (uri, ast) in all_docs {
        collect_implementations(ast, word, uri, &mut locations);
    }
    locations
}

/// Convenience wrapper: extract word at `position` then call `find_implementations`.
pub fn goto_implementation(
    source: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
    position: Position,
) -> Vec<Location> {
    let word = match word_at(source, position) {
        Some(w) => w,
        None => return vec![],
    };
    find_implementations(&word, all_docs)
}

fn collect_implementations(
    stmts: &[Statement],
    word: &str,
    uri: &Url,
    out: &mut Vec<Location>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Class(c) => {
                // Check `extends`
                let extends_match = c
                    .extends
                    .as_ref()
                    .map(|e| e.parent.value.to_string() == word)
                    .unwrap_or(false);

                // Check `implements`
                let implements_match = c
                    .implements
                    .as_ref()
                    .map(|imp| {
                        imp.interfaces
                            .iter()
                            .any(|iface| iface.value.to_string() == word)
                    })
                    .unwrap_or(false);

                if extends_match || implements_match {
                    let start = span_to_position(&c.name.span);
                    let end = Position {
                        line: start.line,
                        character: start.character + c.name.value.to_string().len() as u32,
                    };
                    out.push(Location {
                        uri: uri.clone(),
                        range: Range { start, end },
                    });
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_implementations(inner, word, uri, out);
            }
            _ => {}
        }
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

    fn doc(path: &str, source: &str) -> (Url, Arc<Vec<Statement>>) {
        (uri(path), Arc::new(parse_ast(source)))
    }

    #[test]
    fn finds_class_implementing_interface() {
        let src = "<?php\ninterface Countable {}\nclass MyList implements Countable {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Countable", &docs);
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].range.start.line, 2);
    }

    #[test]
    fn finds_class_extending_parent() {
        let src = "<?php\nclass Animal {}\nclass Dog extends Animal {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Animal", &docs);
        assert_eq!(locs.len(), 1);
    }

    #[test]
    fn no_implementations_for_unknown_name() {
        let src = "<?php\nclass Foo {}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("Bar", &docs);
        assert!(locs.is_empty());
    }

    #[test]
    fn finds_across_multiple_docs() {
        let a = doc("/a.php", "<?php\nclass DogA extends Animal {}");
        let b = doc("/b.php", "<?php\nclass DogB extends Animal {}");
        let locs = find_implementations("Animal", &[a, b]);
        assert_eq!(locs.len(), 2);
    }

    #[test]
    fn class_implementing_multiple_interfaces() {
        let src = "<?php\nclass Repo implements Countable, Serializable {}";
        let docs = vec![doc("/a.php", src)];
        let countable = find_implementations("Countable", &docs);
        let serializable = find_implementations("Serializable", &docs);
        assert_eq!(countable.len(), 1);
        assert_eq!(serializable.len(), 1);
    }

    #[test]
    fn goto_implementation_uses_cursor_word() {
        let src = "<?php\ninterface Countable {}\nclass Repo implements Countable {}";
        let ast = parse_ast(src);
        let docs = vec![(uri("/a.php"), Arc::new(ast))];
        // cursor on "Countable" in the interface declaration line
        let locs = goto_implementation(src, &docs, Position { line: 1, character: 12 });
        assert!(!locs.is_empty());
    }
}
