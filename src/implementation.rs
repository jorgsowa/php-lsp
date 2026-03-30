/// `textDocument/implementation` — find all classes that implement an interface
/// or extend a class with the given name.
use std::sync::Arc;

use php_ast::{NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Url};

use crate::ast::{ParsedDoc, name_range};
use crate::util::word_at;

/// Return all `Location`s where a class declares `extends Name` or
/// `implements Name`.
pub fn find_implementations(word: &str, all_docs: &[(Url, Arc<ParsedDoc>)]) -> Vec<Location> {
    let mut locations = Vec::new();
    for (uri, doc) in all_docs {
        let source = doc.source();
        collect_implementations(&doc.program().stmts, word, source, uri, &mut locations);
    }
    locations
}

/// Convenience wrapper: extract word at `position` then call `find_implementations`.
pub fn goto_implementation(
    source: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Vec<Location> {
    let word = match word_at(source, position) {
        Some(w) => w,
        None => return vec![],
    };
    find_implementations(&word, all_docs)
}

fn collect_implementations(
    stmts: &[Stmt<'_, '_>],
    word: &str,
    source: &str,
    uri: &Url,
    out: &mut Vec<Location>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let extends_match = c
                    .extends
                    .as_ref()
                    .map(|e| e.to_string_repr().as_ref() == word)
                    .unwrap_or(false);

                let implements_match = c
                    .implements
                    .iter()
                    .any(|iface| iface.to_string_repr().as_ref() == word);

                if extends_match || implements_match {
                    if let Some(class_name) = c.name {
                        out.push(Location {
                            uri: uri.clone(),
                            range: name_range(source, class_name),
                        });
                    }
                }
            }
            StmtKind::Enum(e) => {
                let implements_match = e
                    .implements
                    .iter()
                    .any(|iface| iface.to_string_repr().as_ref() == word);
                if implements_match {
                    out.push(Location {
                        uri: uri.clone(),
                        range: name_range(source, e.name),
                    });
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_implementations(inner, word, source, uri, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, source: &str) -> (Url, Arc<ParsedDoc>) {
        (uri(path), Arc::new(ParsedDoc::parse(source.to_string())))
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
        let docs = vec![doc("/a.php", src)];
        let locs = goto_implementation(
            src,
            &docs,
            Position {
                line: 1,
                character: 12,
            },
        );
        assert!(!locs.is_empty());
    }

    #[test]
    fn enum_implementing_interface_is_found() {
        // PHP 8.1+ enums can implement interfaces.
        let src = "<?php\ninterface HasLabel {}\nenum Status: string implements HasLabel {\n    case Active = 'active';\n}";
        let docs = vec![doc("/a.php", src)];
        let locs = find_implementations("HasLabel", &docs);
        assert_eq!(locs.len(), 1, "expected enum Status as implementation of HasLabel, got: {:?}", locs);
        assert_eq!(locs[0].range.start.line, 2, "enum declaration should be on line 2");
    }
}
