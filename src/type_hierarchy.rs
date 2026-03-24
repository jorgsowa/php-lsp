/// `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`.
use std::sync::Arc;

use php_ast::{NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Position, SymbolKind, TypeHierarchyItem, Url};

use crate::ast::{name_range, ParsedDoc};
use crate::util::word_at;

// ── Prepare ───────────────────────────────────────────────────────────────────

pub fn prepare_type_hierarchy(
    source: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Option<TypeHierarchyItem> {
    let word = word_at(source, position)?;
    for (uri, doc) in all_docs {
        let doc_source = doc.source();
        if let Some(item) = find_type_item(doc_source, &doc.program().stmts, &word, uri) {
            return Some(item);
        }
    }
    None
}

fn find_type_item(source: &str, stmts: &[Stmt<'_, '_>], word: &str, uri: &Url) -> Option<TypeHierarchyItem> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(word) => {
                let name = c.name.unwrap();
                return Some(make_item(source, name, SymbolKind::CLASS, uri));
            }
            StmtKind::Interface(i) if i.name == word => {
                return Some(make_item(source, i.name, SymbolKind::INTERFACE, uri));
            }
            StmtKind::Trait(t) if t.name == word => {
                return Some(make_item(source, t.name, SymbolKind::CLASS, uri));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(item) = find_type_item(source, inner, word, uri) {
                        return Some(item);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn make_item(source: &str, name: &str, kind: SymbolKind, uri: &Url) -> TypeHierarchyItem {
    let range = name_range(source, name);
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

pub fn supertypes_of(
    item: &TypeHierarchyItem,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<TypeHierarchyItem> {
    let mut super_names: Vec<String> = Vec::new();

    for (_, doc) in all_docs {
        collect_super_names(&doc.program().stmts, &item.name, &mut super_names);
    }

    let mut result = Vec::new();
    for name in super_names {
        for (uri, doc) in all_docs {
            let doc_source = doc.source();
            if let Some(super_item) = find_type_item(doc_source, &doc.program().stmts, &name, uri) {
                result.push(super_item);
                break;
            }
        }
    }
    result
}

fn collect_super_names(stmts: &[Stmt<'_, '_>], name: &str, out: &mut Vec<String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(name) => {
                if let Some(ext) = &c.extends {
                    out.push(ext.to_string_repr().into_owned());
                }
                for iface in c.implements.iter() {
                    out.push(iface.to_string_repr().into_owned());
                }
            }
            StmtKind::Interface(i) if i.name == name => {
                for parent in i.extends.iter() {
                    out.push(parent.to_string_repr().into_owned());
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_super_names(inner, name, out);
                }
            }
            _ => {}
        }
    }
}

// ── Subtypes ──────────────────────────────────────────────────────────────────

pub fn subtypes_of(
    item: &TypeHierarchyItem,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<TypeHierarchyItem> {
    let mut result = Vec::new();
    for (uri, doc) in all_docs {
        let doc_source = doc.source();
        collect_subtypes(doc_source, &doc.program().stmts, &item.name, uri, &mut result);
    }
    result
}

fn collect_subtypes(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    parent_name: &str,
    uri: &Url,
    out: &mut Vec<TypeHierarchyItem>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let extends_match = c
                    .extends
                    .as_ref()
                    .map(|e| e.to_string_repr().as_ref() == parent_name)
                    .unwrap_or(false);
                let implements_match = c
                    .implements
                    .iter()
                    .any(|i| i.to_string_repr().as_ref() == parent_name);
                if extends_match || implements_match {
                    if let Some(name) = c.name {
                        out.push(make_item(source, name, SymbolKind::CLASS, uri));
                    }
                }
            }
            StmtKind::Interface(i) => {
                let extends_match = i
                    .extends
                    .iter()
                    .any(|p| p.to_string_repr().as_ref() == parent_name);
                if extends_match {
                    out.push(make_item(source, i.name, SymbolKind::INTERFACE, uri));
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_subtypes(source, inner, parent_name, uri, out);
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

    fn doc(path: &str, src: &str) -> (Url, Arc<ParsedDoc>) {
        (uri(path), Arc::new(ParsedDoc::parse(src.to_string())))
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
