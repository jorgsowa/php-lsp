/// `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`.
use std::collections::HashSet;
use std::sync::Arc;

use tower_lsp_server::ls_types::{Position, SymbolKind, TypeHierarchyItem, Uri};

use crate::document::ast::ParsedDoc;
use crate::text::zero_width_range;

fn make_item_from_index(
    name: &str,
    kind: SymbolKind,
    uri: &Uri,
    start_line: u32,
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
        data: None,
    }
}

/// Phase J — Prepare from the salsa-memoized workspace aggregate.
/// Mention-index-narrowed name lookup instead of walking every file's classes.
///
/// `uri` is the document the cursor is in. A short name shared by many
/// classes across the workspace (e.g. Laravel's ~16 `Factory` classes) would
/// otherwise resolve to an arbitrary one via `.first()`. When one of the
/// candidates is declared in `uri` itself — the common case of the cursor
/// sitting on that class's own declaration — it is preferred over an
/// arbitrary first match.
pub fn prepare_type_hierarchy_from_workspace(
    source: &str,
    uri: &Uri,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    position: Position,
    class_candidates_by_short_name: &dyn Fn(&str) -> Vec<crate::db::workspace_index::ClassRef>,
) -> Option<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    use crate::text::word_at_position;
    let word = word_at_position(source, position)?;
    let refs = class_candidates_by_short_name(&word);
    let (uri, cls) = refs
        .iter()
        .filter_map(|r| wi.at(*r))
        .find(|(u, _)| *u == uri)
        .or_else(|| refs.first().and_then(|r| wi.at(*r)))?;
    let kind = match cls.kind {
        ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
        ClassKind::Interface => SymbolKind::INTERFACE,
        ClassKind::Enum => SymbolKind::ENUM,
    };
    Some(make_item_from_index(&cls.name, kind, uri, cls.start_line))
}

/// Phase J — Supertypes via the aggregate. Collect parent/interface names from
/// every declaration of `item.name`, then resolve each name through
/// `class_candidates`. O(definitions-of-item + parents) instead of O(files × classes).
pub fn supertypes_of_from_workspace(
    item: &TypeHierarchyItem,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    class_candidates_by_short_name: &dyn Fn(&str) -> Vec<crate::db::workspace_index::ClassRef>,
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
    resolve_class_ref: &dyn Fn(&str) -> Option<crate::db::workspace_index::ClassRef>,
) -> Vec<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    let mut result = Vec::new();
    let mut seen_fqns: HashSet<Box<str>> = HashSet::new();
    for r in class_candidates_by_short_name(&item.name) {
        let Some((uri, cls)) = wi.at(r) else {
            continue;
        };
        let Some(doc) = get_doc(uri) else {
            continue;
        };
        let imports = doc.file_imports();
        let super_names = cls
            .parent
            .iter()
            .cloned()
            .chain(cls.implements.iter().cloned())
            .chain(cls.traits.iter().cloned());
        for name in super_names {
            let resolved = crate::navigation::moniker::resolve_fqn(&doc, name.as_ref(), &imports);
            let Some((super_uri, super_cls)) =
                resolve_class_ref(&resolved).and_then(|class_ref| wi.at(class_ref))
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
                ));
            }
        }
    }
    result
}

/// Mir-backed variant of [`subtypes_of_from_workspace`].
///
/// `item_fqn` is the FQCN of the hierarchy item (e.g. `"App\\Animal"`),
/// resolved in the handler from the workspace index. `subtype_urls` is the
/// file set from `DocumentStore::class_subtype_urls`. When non-empty this
/// fixes aliased `extends` and FQN-qualified forms the raw-name map misses.
/// Falls back to [`subtypes_of_from_workspace`] when `subtype_urls` is empty.
pub fn subtypes_of_mir_backed(
    item: &TypeHierarchyItem,
    item_fqn: Option<&str>,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    subtype_urls: &[Uri],
    mention_candidates: &dyn Fn(&str) -> Vec<Uri>,
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
) -> Vec<TypeHierarchyItem> {
    if subtype_urls.is_empty() {
        return subtypes_of_from_workspace(item, item_fqn, wi, mention_candidates, get_doc);
    }
    use crate::index::file_index::ClassKind;
    let url_set: HashSet<&Uri> = subtype_urls.iter().collect();
    let mut result = Vec::new();
    for (uri, idx) in &wi.files {
        if !url_set.contains(uri) {
            continue;
        }
        let doc = get_doc(uri);
        let imports = doc.as_ref().map(|doc| doc.file_imports());
        for cls in &idx.classes {
            let matches_name = |name: &str| {
                if let (Some(target_fqn), Some(doc), Some(imports)) =
                    (item_fqn, doc.as_ref(), imports.as_ref())
                {
                    crate::navigation::moniker::resolve_fqn(doc, name, imports)
                        .trim_start_matches('\\')
                        .eq_ignore_ascii_case(target_fqn)
                } else {
                    name == item.name
                        || (item_fqn.is_none()
                            && !item.name.contains('\\')
                            && name.trim_start_matches('\\') == item.name)
                }
            };
            let extends_match = cls.parent.as_deref().is_some_and(matches_name);
            let implements_match = cls.implements.iter().any(|iface| matches_name(iface.as_ref()));
            let uses_match = cls.traits.iter().any(|t| matches_name(t.as_ref()));
            if extends_match || implements_match || uses_match {
                let kind = match cls.kind {
                    ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
                    ClassKind::Interface => SymbolKind::INTERFACE,
                    ClassKind::Enum => SymbolKind::ENUM,
                };
                result.push(make_item_from_index(&cls.name, kind, uri, cls.start_line));
            }
        }
    }
    result
}

/// Phase J — Subtypes via mir's mention index: files that mention
/// `item.name` at all are the only ones whose `extends`/`implements`/`use`
/// clause could possibly name it, so mention-candidates replaces the old
/// eagerly-rebuilt `subtypes_of` reverse map as the narrowing step.
///
/// `item_fqn` is the FQCN of the hierarchy item when known. A mention hit is
/// necessary but not sufficient (over-inclusive across a large workspace,
/// e.g. many unrelated `Factory` interfaces each aliased to the same
/// `FactoryContract` locally), so each candidate is re-checked against
/// `item_fqn` via `resolves_to_fqn`, which resolves the candidate's own
/// `extends`/`implements`/`use` clause through its `use_imports` and
/// namespace before accepting the match. Falls back to a bare short-name
/// match when `item_fqn` is unavailable.
pub fn subtypes_of_from_workspace(
    item: &TypeHierarchyItem,
    item_fqn: Option<&str>,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    mention_candidates: &dyn Fn(&str) -> Vec<Uri>,
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
) -> Vec<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    let candidates: Vec<&(Uri, std::sync::Arc<crate::index::file_index::FileIndex>)> =
        mention_candidates(&item.name)
            .iter()
            .filter_map(|u| {
                let &file_idx = wi.path_to_file_idx.get(u.as_str())?;
                wi.files.get(file_idx as usize)
            })
            .collect();
    candidates
        .iter()
        .flat_map(|(uri, idx)| idx.classes.iter().map(move |cls| (uri, idx.as_ref(), cls)))
        .filter_map(|(uri, file_idx, cls)| {
            let doc = get_doc(uri);
            let imports = doc.as_ref().map(|doc| doc.file_imports());
            let matches = match item_fqn {
                Some(f) => {
                    let named = |name: &str| {
                        if let (Some(doc), Some(imports)) = (doc.as_ref(), imports.as_ref()) {
                            crate::navigation::moniker::resolve_fqn(doc, name, imports)
                                .trim_start_matches('\\')
                                .eq_ignore_ascii_case(f)
                        } else {
                            file_idx
                                .resolve_name_to_fqn(name)
                                .eq_ignore_ascii_case(f)
                        }
                    };
                    cls.parent.as_deref().is_some_and(named)
                        || cls.implements.iter().any(|iface| named(iface.as_ref()))
                        || cls.traits.iter().any(|t| named(t.as_ref()))
                }
                None => {
                    let named = |name: &str| name == item.name;
                    cls.parent.as_deref().is_some_and(named)
                        || cls.implements.iter().any(|iface| named(iface.as_ref()))
                        || cls.traits.iter().any(|t| named(t.as_ref()))
                }
            };
            matches.then_some((uri, cls))
        })
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
