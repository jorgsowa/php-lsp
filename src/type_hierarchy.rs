/// `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`.
///
/// Type hierarchy lets editors navigate the full class/interface inheritance chain.
/// - **prepare**: find the class or interface at the cursor and return a `TypeHierarchyItem`.
/// - **supertypes**: for a class — its `extends` parent and each `implements` interface.
/// - **subtypes**: all classes that extend or implement the given type (reuses implementation scan).
use std::sync::Arc;

use php_parser_rs::parser::ast::{
    namespaces::NamespaceStatement, Statement,
};
use tower_lsp::lsp_types::{
    Position, Range, SymbolKind, TypeHierarchyItem, Url,
};

use crate::diagnostics::span_to_position;
use crate::util::word_at;

// ── Prepare ───────────────────────────────────────────────────────────────────

/// Find the class or interface at `position` and return a `TypeHierarchyItem`.
pub fn prepare_type_hierarchy(
    source: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
    position: Position,
) -> Option<TypeHierarchyItem> {
    let word = word_at(source, position)?;
    for (uri, ast) in all_docs {
        if let Some(item) = find_type_item(ast, &word, uri) {
            return Some(item);
        }
    }
    None
}

fn find_type_item(stmts: &[Statement], word: &str, uri: &Url) -> Option<TypeHierarchyItem> {
    for stmt in stmts {
        match stmt {
            Statement::Class(c) if c.name.value.to_string() == word => {
                return Some(make_item(word, SymbolKind::CLASS, uri, &c.name.span));
            }
            Statement::Interface(i) if i.name.value.to_string() == word => {
                return Some(make_item(word, SymbolKind::INTERFACE, uri, &i.name.span));
            }
            Statement::Trait(t) if t.name.value.to_string() == word => {
                return Some(make_item(word, SymbolKind::CLASS, uri, &t.name.span));
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(item) = find_type_item(inner, word, uri) {
                    return Some(item);
                }
            }
            _ => {}
        }
    }
    None
}

fn make_item(
    name: &str,
    kind: SymbolKind,
    uri: &Url,
    span: &php_parser_rs::lexer::token::Span,
) -> TypeHierarchyItem {
    let start = span_to_position(span);
    let range = Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + name.len() as u32,
        },
    };
    TypeHierarchyItem {
        name: name.to_string(),
        kind,
        tags: None,
        detail: None,
        uri: uri.clone(),
        range,
        selection_range: range,
        data: None,
    }
}

// ── Supertypes ────────────────────────────────────────────────────────────────

/// Return the parent class and implemented interfaces of the given item.
pub fn supertypes_of(
    item: &TypeHierarchyItem,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Vec<TypeHierarchyItem> {
    let mut super_names: Vec<String> = Vec::new();

    // Find the class declaration in indexed docs to get extends/implements names
    for (_, ast) in all_docs {
        collect_super_names(ast, &item.name, &mut super_names);
    }

    // Resolve each super name to a TypeHierarchyItem
    let mut result = Vec::new();
    for name in super_names {
        for (uri, ast) in all_docs {
            if let Some(super_item) = find_type_item(ast, &name, uri) {
                result.push(super_item);
                break;
            }
        }
    }
    result
}

fn collect_super_names(stmts: &[Statement], name: &str, out: &mut Vec<String>) {
    for stmt in stmts {
        match stmt {
            Statement::Class(c) if c.name.value.to_string() == name => {
                if let Some(ext) = &c.extends {
                    out.push(ext.parent.value.to_string());
                }
                if let Some(imp) = &c.implements {
                    for iface in imp.interfaces.iter() {
                        out.push(iface.value.to_string());
                    }
                }
            }
            Statement::Interface(i) if i.name.value.to_string() == name => {
                if let Some(ext) = &i.extends {
                    for parent in ext.parents.iter() {
                        out.push(parent.value.to_string());
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_super_names(inner, name, out);
            }
            _ => {}
        }
    }
}

// ── Subtypes ──────────────────────────────────────────────────────────────────

/// Return all classes that directly extend or implement the given type.
pub fn subtypes_of(
    item: &TypeHierarchyItem,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Vec<TypeHierarchyItem> {
    let mut result = Vec::new();
    for (uri, ast) in all_docs {
        collect_subtypes(ast, &item.name, uri, &mut result);
    }
    result
}

fn collect_subtypes(
    stmts: &[Statement],
    parent_name: &str,
    uri: &Url,
    out: &mut Vec<TypeHierarchyItem>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Class(c) => {
                let extends_match = c
                    .extends
                    .as_ref()
                    .map(|e| e.parent.value.to_string() == parent_name)
                    .unwrap_or(false);
                let implements_match = c
                    .implements
                    .as_ref()
                    .map(|imp| {
                        imp.interfaces
                            .iter()
                            .any(|i| i.value.to_string() == parent_name)
                    })
                    .unwrap_or(false);
                if extends_match || implements_match {
                    let name = c.name.value.to_string();
                    out.push(make_item(&name, SymbolKind::CLASS, uri, &c.name.span));
                }
            }
            Statement::Interface(i) => {
                // An interface that extends this interface is also a subtype
                let extends_match = i
                    .extends
                    .as_ref()
                    .map(|e| e.parents.iter().any(|p| p.value.to_string() == parent_name))
                    .unwrap_or(false);
                if extends_match {
                    let name = i.name.value.to_string();
                    out.push(make_item(&name, SymbolKind::INTERFACE, uri, &i.name.span));
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_subtypes(inner, parent_name, uri, out);
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

    fn doc(path: &str, src: &str) -> (Url, Arc<Vec<Statement>>) {
        (uri(path), Arc::new(parse_ast(src)))
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn prepare_finds_class() {
        let src = "<?php\nclass Foo {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 8));
        assert!(item.is_some());
        assert_eq!(item.unwrap().name, "Foo");
    }

    #[test]
    fn prepare_finds_interface() {
        let src = "<?php\ninterface Countable {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 12));
        assert!(item.is_some());
        assert_eq!(item.as_ref().unwrap().kind, SymbolKind::INTERFACE);
    }

    #[test]
    fn prepare_returns_none_for_unknown() {
        let src = "<?php\n$x = 1;";
        let docs = vec![doc("/a.php", src)];
        assert!(prepare_type_hierarchy(src, &docs, pos(1, 1)).is_none());
    }

    #[test]
    fn supertypes_returns_parent_class() {
        let src = "<?php\nclass Animal {}\nclass Dog extends Animal {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(2, 8)).unwrap();
        let supers = supertypes_of(&item, &docs);
        assert_eq!(supers.len(), 1);
        assert_eq!(supers[0].name, "Animal");
    }

    #[test]
    fn supertypes_returns_implemented_interfaces() {
        let src = "<?php\ninterface Countable {}\ninterface Serializable {}\nclass Repo implements Countable, Serializable {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(3, 8)).unwrap();
        let supers = supertypes_of(&item, &docs);
        assert_eq!(supers.len(), 2);
        let names: Vec<&str> = supers.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"Countable"));
        assert!(names.contains(&"Serializable"));
    }

    #[test]
    fn supertypes_of_top_level_is_empty() {
        let src = "<?php\nclass Root {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 8)).unwrap();
        let supers = supertypes_of(&item, &docs);
        assert!(supers.is_empty());
    }

    #[test]
    fn subtypes_finds_implementing_class() {
        let src = "<?php\ninterface Countable {}\nclass MyList implements Countable {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 12)).unwrap();
        let subs = subtypes_of(&item, &docs);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].name, "MyList");
    }

    #[test]
    fn subtypes_finds_extending_class() {
        let src = "<?php\nclass Animal {}\nclass Dog extends Animal {}\nclass Cat extends Animal {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 8)).unwrap();
        let subs = subtypes_of(&item, &docs);
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn subtypes_cross_file() {
        let base = doc("/base.php", "<?php\nclass Animal {}");
        let child = doc("/child.php", "<?php\nclass Dog extends Animal {}");
        let docs = vec![base, child];
        let item = prepare_type_hierarchy(
            "<?php\nclass Animal {}",
            &docs,
            pos(1, 8),
        ).unwrap();
        let subs = subtypes_of(&item, &docs);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].name, "Dog");
    }
}
