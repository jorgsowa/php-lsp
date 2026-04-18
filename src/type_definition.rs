/// `textDocument/typeDefinition` — jump to the class declaration of the type
/// of the symbol under the cursor.
///
/// Works for variables assigned via `$var = new ClassName()` (leverages `TypeMap`)
/// and for function parameters with a declared type hint.
use std::sync::Arc;

use php_ast::{NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::ast::{ParsedDoc, SourceView, format_type_hint};
use crate::type_map::TypeMap;
use crate::util::word_at;

/// Given the cursor position, resolve the type of the symbol and return the
/// location of that type's class/interface declaration.
pub fn goto_type_definition(
    source: &str,
    doc: &ParsedDoc,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at(source, position)?;

    let type_map = TypeMap::from_doc(doc);
    let class_name = if word.starts_with('$') {
        type_map.get(&word)?.to_string()
    } else {
        param_type_for(&doc.program().stmts, &word)?
    };

    for (uri, other_doc) in all_docs {
        let other_sv = other_doc.view();
        if let Some(range) = find_class_range(other_sv, &other_doc.program().stmts, &class_name) {
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }
    None
}

/// Look up the declared type hint for a parameter named `word` in any function/method.
fn param_type_for(stmts: &[Stmt<'_, '_>], word: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                for p in f.params.iter() {
                    if p.name == word
                        && let Some(t) = &p.type_hint
                    {
                        return Some(format_type_hint(t));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(t) = param_type_for(inner, word)
                {
                    return Some(t);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the range of the class or interface declaration named `name`.
fn find_class_range(sv: SourceView<'_>, stmts: &[Stmt<'_, '_>], name: &str) -> Option<Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(name) => {
                return Some(sv.name_range(c.name.expect("match guard ensures Some")));
            }
            StmtKind::Interface(i) if i.name == name => {
                return Some(sv.name_range(i.name));
            }
            StmtKind::Trait(t) if t.name == name => {
                return Some(sv.name_range(t.name));
            }
            StmtKind::Enum(e) if e.name == name => {
                return Some(sv.name_range(e.name));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_class_range(sv, inner, name)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn _offset_to_position_range(sv: SourceView<'_>, name_str: &str, _name: &str) -> Range {
    let start = sv.position_of(0);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character
                + name_str.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn doc(path: &str, src: &str) -> (Url, Arc<ParsedDoc>) {
        (uri(path), Arc::new(ParsedDoc::parse(src.to_string())))
    }

    #[test]
    fn resolves_variable_type_to_class() {
        let src = "<?php\nclass Foo {}\n$obj = new Foo();\n$obj->bar();";
        let parsed = ParsedDoc::parse(src.to_string());
        let docs = vec![(uri("/a.php"), Arc::new(ParsedDoc::parse(src.to_string())))];
        let loc = goto_type_definition(src, &parsed, &docs, pos(3, 2));
        assert!(loc.is_some(), "expected type definition for $obj");
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn cross_file_type_definition() {
        let src = "<?php\n$obj = new Mailer();\n$obj->send();";
        let parsed = ParsedDoc::parse(src.to_string());
        let other_src = "<?php\nclass Mailer {}";
        let other_uri = uri("/mailer.php");
        let docs = vec![
            doc("/a.php", src),
            (
                other_uri.clone(),
                Arc::new(ParsedDoc::parse(other_src.to_string())),
            ),
        ];
        let loc = goto_type_definition(src, &parsed, &docs, pos(2, 2));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().uri, other_uri);
    }

    #[test]
    fn unknown_variable_returns_none() {
        let src = "<?php\n$unknown->foo();";
        let parsed = ParsedDoc::parse(src.to_string());
        let docs = vec![doc("/a.php", src)];
        let loc = goto_type_definition(src, &parsed, &docs, pos(1, 2));
        assert!(loc.is_none());
    }

    #[test]
    fn resolves_interface_type() {
        let src = "<?php\ninterface Countable {}\n$obj = new MyList();\nclass MyList implements Countable {}";
        let parsed = ParsedDoc::parse(src.to_string());
        let docs = vec![doc("/a.php", src)];
        let loc = goto_type_definition(src, &parsed, &docs, pos(2, 2));
        assert!(loc.is_some());
        assert_eq!(loc.unwrap().range.start.line, 3);
    }

    #[test]
    fn returns_none_for_non_variable_without_type() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let parsed = ParsedDoc::parse(src.to_string());
        let docs = vec![doc("/a.php", src)];
        let loc = goto_type_definition(src, &parsed, &docs, pos(2, 2));
        assert!(loc.is_none());
    }

    #[test]
    fn resolves_enum_typed_param() {
        // Cursor on `$s` in the function body — TypeMap infers Status from the typed param.
        let src = "<?php\nenum Status { case Active; }\nfunction process(Status $s): void { $s-> }";
        let parsed = ParsedDoc::parse(src.to_string());
        let docs = vec![doc("/a.php", src)];
        // "function process(Status $s): void { " is 37 chars, so $s is at col 37.
        let loc = goto_type_definition(src, &parsed, &docs, pos(2, 37));
        assert!(
            loc.is_some(),
            "expected type definition for Status-typed param"
        );
        assert_eq!(loc.unwrap().range.start.line, 1);
    }

    #[test]
    fn resolves_trait_typed_param() {
        // Cursor on `$l` in the function body — TypeMap infers Logger from the typed param.
        let src = "<?php\ntrait Logger {}\nfunction process(Logger $l): void { $l-> }";
        let parsed = ParsedDoc::parse(src.to_string());
        let docs = vec![doc("/a.php", src)];
        // "function process(Logger $l): void { " is 37 chars, so $l is at col 37.
        let loc = goto_type_definition(src, &parsed, &docs, pos(2, 37));
        assert!(
            loc.is_some(),
            "expected type definition for trait-typed param"
        );
        assert_eq!(loc.unwrap().range.start.line, 1);
    }
}
