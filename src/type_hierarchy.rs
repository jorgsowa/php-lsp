/// `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`.
use std::sync::Arc;

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Position, SymbolKind, TypeHierarchyItem, Url};

use crate::ast::{ParsedDoc, SourceView};
use crate::util::word_at;

// ── Prepare ───────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn prepare_type_hierarchy(
    source: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    position: Position,
) -> Option<TypeHierarchyItem> {
    let word = word_at(source, position)?;
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(item) = find_type_item(sv, &doc.program().stmts, &word, uri) {
            return Some(item);
        }
    }
    None
}

#[allow(dead_code)]
fn find_type_item(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    word: &str,
    uri: &Url,
) -> Option<TypeHierarchyItem> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(word) => {
                let name = c.name.expect("match guard ensures Some");
                return Some(make_item(sv, name, SymbolKind::CLASS, uri));
            }
            StmtKind::Interface(i) if i.name == word => {
                return Some(make_item(sv, i.name, SymbolKind::INTERFACE, uri));
            }
            StmtKind::Trait(t) if t.name == word => {
                return Some(make_item(sv, t.name, SymbolKind::CLASS, uri));
            }
            StmtKind::Enum(e) if e.name == word => {
                return Some(make_item(sv, e.name, SymbolKind::ENUM, uri));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(item) = find_type_item(sv, inner, word, uri)
                {
                    return Some(item);
                }
            }
            _ => {}
        }
    }
    None
}

#[allow(dead_code)]
fn make_item(sv: SourceView<'_>, name: &str, kind: SymbolKind, uri: &Url) -> TypeHierarchyItem {
    let range = sv.name_range(name);
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

#[allow(dead_code)]
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
            let sv = doc.view();
            if let Some(super_item) = find_type_item(sv, &doc.program().stmts, &name, uri) {
                result.push(super_item);
                break;
            }
        }
    }
    result
}

#[allow(dead_code)]
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
            StmtKind::Enum(e) if e.name == name => {
                for iface in e.implements.iter() {
                    out.push(iface.to_string_repr().into_owned());
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

#[allow(dead_code)]
pub fn subtypes_of(
    item: &TypeHierarchyItem,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<TypeHierarchyItem> {
    let mut result = Vec::new();
    for (uri, doc) in all_docs {
        let sv = doc.view();
        collect_subtypes(sv, &doc.program().stmts, &item.name, uri, &mut result);
    }
    result
}

#[allow(dead_code)]
fn collect_subtypes(
    sv: SourceView<'_>,
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
                let trait_use_match = c.members.iter().any(|m| {
                    if let ClassMemberKind::TraitUse(tu) = &m.kind {
                        tu.traits
                            .iter()
                            .any(|t| t.to_string_repr().as_ref() == parent_name)
                    } else {
                        false
                    }
                });
                if (extends_match || implements_match || trait_use_match)
                    && let Some(name) = c.name
                {
                    out.push(make_item(sv, name, SymbolKind::CLASS, uri));
                }
            }
            StmtKind::Interface(i) => {
                let extends_match = i
                    .extends
                    .iter()
                    .any(|p| p.to_string_repr().as_ref() == parent_name);
                if extends_match {
                    out.push(make_item(sv, i.name, SymbolKind::INTERFACE, uri));
                }
            }
            StmtKind::Enum(e) => {
                let implements_match = e
                    .implements
                    .iter()
                    .any(|i| i.to_string_repr().as_ref() == parent_name);
                if implements_match {
                    out.push(make_item(sv, e.name, SymbolKind::ENUM, uri));
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_subtypes(sv, inner, parent_name, uri, out);
                }
            }
            _ => {}
        }
    }
}

// ── Index-based variants ──────────────────────────────────────────────────────

fn line_range(line: u32) -> tower_lsp::lsp_types::Range {
    let pos = Position { line, character: 0 };
    tower_lsp::lsp_types::Range {
        start: pos,
        end: pos,
    }
}

fn make_item_from_index(
    name: &str,
    kind: SymbolKind,
    uri: &Url,
    start_line: u32,
) -> TypeHierarchyItem {
    let range = line_range(start_line);
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

/// Phase J — Prepare from the salsa-memoized workspace aggregate. Constant-time
/// name lookup via `classes_by_name` instead of walking every file's classes.
pub fn prepare_type_hierarchy_from_workspace(
    source: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    position: Position,
) -> Option<TypeHierarchyItem> {
    use crate::file_index::ClassKind;
    use crate::util::word_at;
    let word = word_at(source, position)?;
    let refs = wi.classes_by_name.get(&word)?;
    let (uri, cls) = wi.at(*refs.first()?)?;
    let kind = match cls.kind {
        ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
        ClassKind::Interface => SymbolKind::INTERFACE,
        ClassKind::Enum => SymbolKind::ENUM,
    };
    Some(make_item_from_index(&cls.name, kind, uri, cls.start_line))
}

/// Phase J — Supertypes via the aggregate. Collect parent/interface names from
/// every declaration of `item.name`, then resolve each name through
/// `classes_by_name`. O(definitions-of-item + parents) instead of O(files × classes).
pub fn supertypes_of_from_workspace(
    item: &TypeHierarchyItem,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
) -> Vec<TypeHierarchyItem> {
    use crate::file_index::ClassKind;
    let mut super_names: Vec<String> = Vec::new();
    if let Some(refs) = wi.classes_by_name.get(&item.name) {
        for r in refs {
            if let Some((_, cls)) = wi.at(*r) {
                if let Some(p) = &cls.parent {
                    super_names.push(p.clone());
                }
                for iface in &cls.implements {
                    super_names.push(iface.clone());
                }
            }
        }
    }

    let mut result = Vec::new();
    for name in super_names {
        if let Some(refs) = wi.classes_by_name.get(&name)
            && let Some((uri, cls)) = refs.first().and_then(|r| wi.at(*r))
        {
            let kind = match cls.kind {
                ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
                ClassKind::Interface => SymbolKind::INTERFACE,
                ClassKind::Enum => SymbolKind::ENUM,
            };
            result.push(make_item_from_index(&cls.name, kind, uri, cls.start_line));
        }
    }
    result
}

/// Phase J — Subtypes via the pre-built `subtypes_of` reverse map. O(matches)
/// instead of O(files × classes).
pub fn subtypes_of_from_workspace(
    item: &TypeHierarchyItem,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
) -> Vec<TypeHierarchyItem> {
    use crate::file_index::ClassKind;
    let Some(refs) = wi.subtypes_of.get(&item.name) else {
        return Vec::new();
    };
    refs.iter()
        .filter_map(|r| wi.at(*r))
        .map(|(uri, cls)| {
            let kind = match cls.kind {
                ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
                ClassKind::Interface => SymbolKind::INTERFACE,
                ClassKind::Enum => SymbolKind::ENUM,
            };
            make_item_from_index(&cls.name, kind, uri, cls.start_line)
        })
        .collect()
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
        let src =
            "<?php\nclass Animal {}\nclass Dog extends Animal {}\nclass Cat extends Animal {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 8)).unwrap();
        let subs = subtypes_of(&item, &docs);
        assert_eq!(subs.len(), 2);
    }

    #[test]
    fn prepare_finds_enum() {
        let src = "<?php\nenum Suit { case Hearts; }";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 7));
        assert!(item.is_some(), "expected type hierarchy item for enum");
        assert_eq!(item.as_ref().unwrap().name, "Suit");
        assert_eq!(item.unwrap().kind, SymbolKind::ENUM);
    }

    #[test]
    fn supertypes_of_enum_returns_implemented_interfaces() {
        let src =
            "<?php\ninterface Labelable {}\nenum Status implements Labelable { case Active; }";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(2, 7)).unwrap();
        let supers = supertypes_of(&item, &docs);
        assert_eq!(supers.len(), 1, "expected 1 supertype (Labelable)");
        assert_eq!(supers[0].name, "Labelable");
    }

    #[test]
    fn subtypes_finds_implementing_enum() {
        let src =
            "<?php\ninterface Labelable {}\nenum Status implements Labelable { case Active; }";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 12)).unwrap();
        let subs = subtypes_of(&item, &docs);
        assert_eq!(subs.len(), 1, "expected enum Status as subtype");
        assert_eq!(subs[0].name, "Status");
        assert_eq!(subs[0].kind, SymbolKind::ENUM);
    }

    #[test]
    fn subtypes_cross_file() {
        let base = doc("/base.php", "<?php\nclass Animal {}");
        let child = doc("/child.php", "<?php\nclass Dog extends Animal {}");
        let docs = vec![base, child];
        let item = prepare_type_hierarchy("<?php\nclass Animal {}", &docs, pos(1, 8)).unwrap();
        let subs = subtypes_of(&item, &docs);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].name, "Dog");
    }

    #[test]
    fn prepare_finds_trait_with_class_kind() {
        let src = "<?php\ntrait Loggable {}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 8));
        assert!(item.is_some(), "expected type hierarchy item for trait");
        assert_eq!(item.as_ref().unwrap().name, "Loggable");
        // Traits use CLASS (not INTERFACE) — LSP has no dedicated trait kind.
        assert_eq!(item.unwrap().kind, SymbolKind::CLASS);
    }

    #[test]
    fn subtypes_finds_class_using_trait() {
        let src = "<?php\ntrait Loggable {}\nclass Service {\n    use Loggable;\n}";
        let docs = vec![doc("/a.php", src)];
        let item = prepare_type_hierarchy(src, &docs, pos(1, 8)).unwrap();
        let subs = subtypes_of(&item, &docs);
        assert_eq!(
            subs.len(),
            1,
            "expected Service as subtype of trait Loggable"
        );
        assert_eq!(subs[0].name, "Service");
        assert_eq!(subs[0].kind, SymbolKind::CLASS);
    }
}
