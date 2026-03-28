use std::sync::Arc;

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{ParsedDoc, name_range, offset_to_position, str_offset};
use crate::util::word_at;

/// Find the definition of the symbol under `position`.
/// Searches the current document first, then `other_docs` for cross-file resolution.
pub fn goto_definition(
    uri: &Url,
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at(source, position)?;

    if let Some(range) = scan_statements(source, &doc.program().stmts, &word) {
        return Some(Location {
            uri: uri.clone(),
            range,
        });
    }

    for (other_uri, other_doc) in other_docs {
        let other_source = other_doc.source();
        if let Some(range) = scan_statements(other_source, &other_doc.program().stmts, &word) {
            return Some(Location {
                uri: other_uri.clone(),
                range,
            });
        }
    }

    None
}

/// Search an AST for a declaration named `name`, returning its selection range.
/// Used by the PSR-4 fallback in the backend after resolving a class to a file.
pub fn find_declaration_range(source: &str, doc: &ParsedDoc, name: &str) -> Option<Range> {
    scan_statements(source, &doc.program().stmts, name)
}

fn scan_statements(source: &str, stmts: &[Stmt<'_, '_>], word: &str) -> Option<Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(name_range(source, f.name));
            }
            StmtKind::Class(c) if c.name == Some(word) => {
                let name = c.name.unwrap();
                return Some(name_range(source, name));
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
            StmtKind::Enum(e) if e.name == word => {
                return Some(name_range(source, e.name));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(range) = scan_statements(source, inner, word) {
                        return Some(range);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn _name_range_from_offset(source: &str, name: &str) -> Range {
    let start_offset = str_offset(source, name);
    let start = offset_to_position(source, start_offset);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + name.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
        },
    }
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

    #[test]
    fn jumps_to_function_definition() {
        let src = "<?php\nfunction greet() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 10));
        assert!(result.is_some(), "expected a location");
        let loc = result.unwrap();
        assert_eq!(loc.range.start.line, 1);
        assert_eq!(loc.uri, uri());
    }

    #[test]
    fn jumps_to_class_definition() {
        let src = "<?php\nclass MyService {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 8));
        assert!(result.is_some());
        let loc = result.unwrap();
        assert_eq!(loc.range.start.line, 1);
    }

    #[test]
    fn jumps_to_interface_definition() {
        let src = "<?php\ninterface Countable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 12));
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 1);
    }

    #[test]
    fn jumps_to_trait_definition() {
        let src = "<?php\ntrait Loggable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 8));
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 1);
    }

    #[test]
    fn jumps_to_class_method_definition() {
        let src = "<?php\nclass Calc { public function add() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 32));
        assert!(result.is_some(), "expected location for method 'add'");
    }

    #[test]
    fn returns_none_for_unknown_word() {
        let src = "<?php\n$x = 1;";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(1, 1));
        assert!(result.is_none());
    }

    #[test]
    fn jumps_to_symbol_inside_namespace() {
        let src = "<?php\nnamespace App {\nfunction boot() {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = goto_definition(&uri(), src, &doc, &[], pos(2, 10));
        assert!(result.is_some());
        assert_eq!(result.unwrap().range.start.line, 2);
    }

    #[test]
    fn finds_class_definition_in_other_document() {
        let current_src = "<?php\n$s = new MyService();";
        let current_doc = ParsedDoc::parse(current_src.to_string());
        let other_src = "<?php\nclass MyService {}";
        let other_uri = Url::parse("file:///other.php").unwrap();
        let other_doc = Arc::new(ParsedDoc::parse(other_src.to_string()));

        let result = goto_definition(
            &uri(),
            current_src,
            &current_doc,
            &[(other_uri.clone(), other_doc)],
            pos(1, 13),
        );
        assert!(result.is_some(), "expected cross-file location");
        assert_eq!(result.unwrap().uri, other_uri);
    }

    #[test]
    fn finds_function_definition_in_other_document() {
        let current_src = "<?php\nhelperFn();";
        let current_doc = ParsedDoc::parse(current_src.to_string());
        let other_src = "<?php\nfunction helperFn() {}";
        let other_uri = Url::parse("file:///helpers.php").unwrap();
        let other_doc = Arc::new(ParsedDoc::parse(other_src.to_string()));

        let result = goto_definition(
            &uri(),
            current_src,
            &current_doc,
            &[(other_uri.clone(), other_doc)],
            pos(1, 3),
        );
        assert!(
            result.is_some(),
            "expected cross-file location for helperFn"
        );
        assert_eq!(result.unwrap().uri, other_uri);
    }

    #[test]
    fn current_file_takes_priority_over_other_docs() {
        let src = "<?php\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        let other_src = "<?php\nclass Foo {}";
        let other_uri = Url::parse("file:///other.php").unwrap();
        let other_doc = Arc::new(ParsedDoc::parse(other_src.to_string()));

        let result = goto_definition(&uri(), src, &doc, &[(other_uri, other_doc)], pos(1, 8));
        assert_eq!(result.unwrap().uri, uri(), "should prefer current file");
    }
}
