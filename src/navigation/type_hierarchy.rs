/// `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`.
use std::collections::HashSet;
use std::sync::Arc;

use tower_lsp_server::ls_types::{Position, SymbolKind, TypeHierarchyItem, Uri};

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
    class_candidates: &dyn Fn(&str) -> Vec<crate::db::workspace_index::ClassRef>,
) -> Option<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    use crate::text::word_at_position;
    let word = word_at_position(source, position)?;
    let refs = class_candidates(&word);
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
    class_candidates: &dyn Fn(&str) -> Vec<crate::db::workspace_index::ClassRef>,
) -> Vec<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    // Collect (super_name_as_written, file_index_for_that_class) pairs.
    let mut super_pairs: Vec<(Arc<str>, Option<&crate::index::file_index::FileIndex>)> = Vec::new();
    for r in class_candidates(&item.name) {
        if let Some((_, cls)) = wi.at(r) {
            let file_idx = wi.files.get(r.file as usize).map(|(_, idx)| idx.as_ref());
            if let Some(p) = &cls.parent {
                super_pairs.push((Arc::clone(p), file_idx));
            }
            for iface in &cls.implements {
                super_pairs.push((Arc::clone(iface), file_idx));
            }
            for used_trait in &cls.traits {
                super_pairs.push((Arc::clone(used_trait), file_idx));
            }
        }
    }

    let mut result = Vec::new();
    // Distinct classes sharing a name (e.g. two `BlogController`s in
    // different namespaces) each contribute their own `super_pairs` entries
    // above; when both extend the same parent, that parent would otherwise
    // be pushed twice. Dedup on FQN to collapse those repeats without
    // narrowing which parents count.
    let mut seen_fqns: HashSet<Box<str>> = HashSet::new();
    for (name, file_idx) in super_pairs {
        // Direct lookup: class is named exactly as written.
        let direct = class_candidates(name.as_ref());
        let canonical = if !direct.is_empty() {
            Some(name.as_ref().to_string())
        } else {
            // Resolve through the implementing file's use_imports.
            file_idx.and_then(|idx| {
                idx.use_imports
                    .iter()
                    .find(|(alias, _)| alias.as_ref() == name.as_ref())
                    .map(|(_, fqn)| crate::text::fqn_short_name(fqn).to_string())
            })
        };
        let Some(canonical_name) = canonical else {
            continue;
        };
        let refs = class_candidates(&canonical_name);
        if let Some((uri, cls)) = refs.first().and_then(|r| wi.at(*r))
            && seen_fqns.insert(cls.fqn.clone())
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
) -> Vec<TypeHierarchyItem> {
    if subtype_urls.is_empty() {
        return subtypes_of_from_workspace(item, item_fqn, wi, mention_candidates);
    }
    use crate::index::file_index::ClassKind;
    use crate::navigation::implementation::{alias_resolves_to, name_matches};
    let url_set: HashSet<&Uri> = subtype_urls.iter().collect();
    let mut result = Vec::new();
    for (uri, idx) in &wi.files {
        if !url_set.contains(uri) {
            continue;
        }
        for cls in &idx.classes {
            let extends_match = cls
                .parent
                .as_deref()
                .map(|p| {
                    name_matches(p, &item.name, item_fqn)
                        || item_fqn.is_some_and(|f| alias_resolves_to(p, f, &idx.use_imports))
                })
                .unwrap_or(false);
            let implements_match = cls.implements.iter().any(|iface| {
                name_matches(iface.as_ref(), &item.name, item_fqn)
                    || item_fqn
                        .is_some_and(|f| alias_resolves_to(iface.as_ref(), f, &idx.use_imports))
            });
            let uses_match = cls.traits.iter().any(|t| {
                name_matches(t.as_ref(), &item.name, item_fqn)
                    || item_fqn.is_some_and(|f| alias_resolves_to(t.as_ref(), f, &idx.use_imports))
            });
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
) -> Vec<TypeHierarchyItem> {
    use crate::index::file_index::ClassKind;
    use crate::navigation::implementation::resolves_to_fqn;
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
            let matches = match item_fqn {
                Some(f) => {
                    let named = |name: &str| resolves_to_fqn(name, f, file_idx);
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
