/// `textDocument/declaration` — jump to the abstract or interface declaration of a symbol.
///
/// In PHP the distinction between declaration and definition matters for:
///   - Interface methods (declared but never given a body)
///   - Abstract class methods
///
/// For concrete symbols with no abstract counterpart this falls back to the same
/// result as go-to-definition so the request is never empty-handed.
use std::sync::Arc;

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Url};

use crate::ast::{ParsedDoc, name_range};
use crate::util::word_at;

/// Find the abstract or interface declaration of `word`.
/// Prefers abstract/interface declarations; falls back to any declaration.
pub fn goto_declaration(
    source: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at(source, position)?;

    // First pass: look for an abstract or interface declaration
    for (uri, doc) in all_docs {
        let doc_source = doc.source();
        if let Some(range) = find_abstract_declaration(doc_source, &doc.program().stmts, &word) {
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    // Second pass: any declaration (same as goto_definition)
    for (uri, doc) in all_docs {
        let doc_source = doc.source();
        if let Some(range) = find_any_declaration(doc_source, &doc.program().stmts, &word) {
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    None
}

fn find_abstract_declaration(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<tower_lsp::lsp_types::Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Interface(i) => {
                // Interface methods are declarations without bodies
                for member in i.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            return Some(name_range(source, m.name));
                        }
                    }
                }
                if i.name == word {
                    return Some(name_range(source, i.name));
                }
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.is_abstract && m.name == word {
                            return Some(name_range(source, m.name));
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(r) = find_abstract_declaration(source, inner, word) {
                        return Some(r);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn find_any_declaration(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<tower_lsp::lsp_types::Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(name_range(source, f.name));
            }
            StmtKind::Class(c) if c.name == Some(word) => {
                return Some(name_range(source, c.name.unwrap()));
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            return Some(name_range(source, m.name));
                        }
                    }
                }
            }
            StmtKind::Interface(i) if i.name == word => {
                return Some(name_range(source, i.name));
            }
            StmtKind::Trait(t) if t.name == word => {
                return Some(name_range(source, t.name));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(r) = find_any_declaration(source, inner, word) {
                        return Some(r);
                    }
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

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, src: &str) -> (Url, Arc<ParsedDoc>) {
        (uri(path), Arc::new(ParsedDoc::parse(src.to_string())))
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn finds_interface_method_declaration() {
        let src = "<?php\ninterface Logger { public function log(string $msg): void; }\nclass FileLogger implements Logger { public function log(string $msg): void {} }";
        let docs = vec![doc("/a.php", src)];
        let loc = goto_declaration(src, &docs, pos(2, 53));
        assert!(loc.is_some(), "expected a declaration location");
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
        let impl_src =
            "<?php\nclass Repo implements Countable { public function count(): int { return 0; } }";
        let iface_src = "<?php\ninterface Countable { public function count(): int; }";
        let iface_uri = uri("/iface.php");
        let docs = vec![
            doc("/impl.php", impl_src),
            (
                iface_uri.clone(),
                Arc::new(ParsedDoc::parse(iface_src.to_string())),
            ),
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
