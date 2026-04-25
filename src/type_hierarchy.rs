/// `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`.
use std::sync::Arc;

use tower_lsp::lsp_types::{Position, SymbolKind, TypeHierarchyItem, Url};

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
    let mut super_names: Vec<Arc<str>> = Vec::new();
    if let Some(refs) = wi.classes_by_name.get(&item.name) {
        for r in refs {
            if let Some((_, cls)) = wi.at(*r) {
                if let Some(p) = &cls.parent {
                    super_names.push(Arc::clone(p));
                }
                for iface in &cls.implements {
                    super_names.push(Arc::clone(iface));
                }
            }
        }
    }

    let mut result = Vec::new();
    for name in super_names {
        if let Some(refs) = wi.classes_by_name.get(name.as_ref())
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
    let Some(refs) = wi.subtypes_of.get(item.name.as_str()) else {
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
