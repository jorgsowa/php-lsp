/// `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`.
use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};
use tower_lsp_server::ls_types::{SymbolKind, TypeHierarchyItem, Uri};

use crate::document::ast::ParsedDoc;
use crate::text::zero_width_range;

fn make_item_from_index(
    name: &str,
    kind: SymbolKind,
    uri: &Uri,
    start_line: u32,
    fqn: &str,
) -> TypeHierarchyItem {
    let range = zero_width_range(start_line);
    TypeHierarchyItem {
        name: name.to_string(),
        kind,
        tags: None,
        detail: None,
        uri: uri.clone(),
        range,
        selection_range: range,
        data: Some(json!({ "fqn": fqn.trim_start_matches('\\') })),
    }
}

fn sort_items_stably(items: &mut [TypeHierarchyItem]) {
    items.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.uri.as_str().cmp(b.uri.as_str()))
            .then_with(|| a.range.start.line.cmp(&b.range.start.line))
            .then_with(|| a.range.start.character.cmp(&b.range.start.character))
    });
}

pub fn item_fqn(item: &TypeHierarchyItem) -> Option<&str> {
    item.data
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|obj| obj.get("fqn"))
        .and_then(Value::as_str)
}

/// Phase J — Prepare from the salsa-memoized workspace aggregate.
/// Build a hierarchy item from its canonical FQN.
pub fn prepare_type_hierarchy_from_fqn(
    class_ref: crate::db::workspace_index::ClassRef,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
) -> Option<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    let (uri, cls) = wi.at(class_ref)?;
    let kind = match cls.kind {
        ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
        ClassKind::Interface => SymbolKind::INTERFACE,
        ClassKind::Enum => SymbolKind::ENUM,
    };
    Some(make_item_from_index(
        &cls.name,
        kind,
        uri,
        cls.start_line,
        &cls.fqn,
    ))
}

/// Hierarchy items for `supertype_fqns` (direct parent, interfaces, traits as
/// resolved by mir) that are declared in the workspace.
pub fn supertypes_of_from_workspace(
    supertype_fqns: &[Arc<str>],
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    resolve_class_ref: &dyn Fn(&str) -> Option<crate::db::workspace_index::ClassRef>,
) -> Vec<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    let mut result = Vec::new();
    let mut seen_fqns: HashSet<Box<str>> = HashSet::new();
    for fqn in supertype_fqns {
        let Some((super_uri, super_cls)) =
            resolve_class_ref(fqn).and_then(|class_ref| wi.at(class_ref))
        else {
            continue;
        };
        if seen_fqns.insert(super_cls.fqn.clone()) {
            let kind = match super_cls.kind {
                ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
                ClassKind::Interface => SymbolKind::INTERFACE,
                ClassKind::Enum => SymbolKind::ENUM,
            };
            result.push(make_item_from_index(
                &super_cls.name,
                kind,
                super_uri,
                super_cls.start_line,
                &super_cls.fqn,
            ));
        }
    }
    result
}

/// Direct subtypes of `item_fqn` among the classes declared in `subtype_urls`
/// (mir's subtype-edge files, including trait users).
pub fn subtypes_of_mir_backed(
    item_fqn: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    subtype_urls: &[Uri],
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
) -> Vec<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    let mut result = Vec::new();
    wi.for_each_class_in_uris(subtype_urls, |uri, cls| {
        let doc = get_doc(uri);
        let imports = doc.as_ref().map(|doc| doc.file_imports());
        let matches_name = |name: &str| {
            if let (Some(doc), Some(imports)) = (doc.as_ref(), imports.as_ref()) {
                crate::navigation::moniker::resolve_fqn(doc, name, imports)
                    .trim_start_matches('\\')
                    .eq_ignore_ascii_case(item_fqn)
            } else {
                false
            }
        };
        let extends_match = cls.parent.as_deref().is_some_and(matches_name);
        let implements_match = cls
            .implements
            .iter()
            .any(|iface| matches_name(iface.as_ref()));
        let uses_match = cls.traits.iter().any(|t| matches_name(t.as_ref()));
        if extends_match || implements_match || uses_match {
            let kind = match cls.kind {
                ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
                ClassKind::Interface => SymbolKind::INTERFACE,
                ClassKind::Enum => SymbolKind::ENUM,
            };
            result.push(make_item_from_index(
                &cls.name,
                kind,
                uri,
                cls.start_line,
                &cls.fqn,
            ));
        }
    });
    sort_items_stably(&mut result);
    result
}
