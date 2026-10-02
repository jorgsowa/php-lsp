/// `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`.
use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};
use tower_lsp_server::ls_types::{SymbolKind, TypeHierarchyItem, Uri};

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

/// Direct subtypes (`extends`, `implements` or `use`) from mir's subtype index.
pub fn subtypes_from_sites(sites: &[mir_analyzer::SubtypeClassSite]) -> Vec<TypeHierarchyItem> {
    use mir_analyzer::db::ClassLikeKind;
    let mut result: Vec<TypeHierarchyItem> = sites
        .iter()
        // Anonymous classes are indexed as `class@anonymous…` and have no name to show.
        .filter(|site| !site.fqcn.contains('@'))
        .filter_map(|site| {
            let uri = site.file.parse::<Uri>().ok()?;
            let kind = match site.kind {
                ClassLikeKind::Class | ClassLikeKind::Trait => SymbolKind::CLASS,
                ClassLikeKind::Interface => SymbolKind::INTERFACE,
                ClassLikeKind::Enum => SymbolKind::ENUM,
            };
            Some(make_item_from_index(
                crate::text::fqn_short_name(&site.fqcn),
                kind,
                &uri,
                site.range.start.line.saturating_sub(1),
                &site.fqcn,
            ))
        })
        .collect();
    sort_items_stably(&mut result);
    result
}
