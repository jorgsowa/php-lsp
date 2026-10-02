use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{
    ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind, TraitAdaptationKind,
};
use tower_lsp_server::ls_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind, Uri,
};

use crate::document::ast::{ParsedDoc, SourceView};

/// Finds the declaration matching `name` and returns a `CallHierarchyItem`,
/// narrowing candidate declaring files via mir's persistent per-file mention
/// cache (`mention_candidates`) instead of walking every document's AST:
/// O(matches) docs fetched and scanned instead of O(workspace). A candidate
/// is a *possible* declarer (mention, not proof) — `find_declaration_item`
/// below still does the real AST-level check.
/// `get_doc` resolves a candidate file to its parsed doc (typically
/// `DocumentStore::get_doc_salsa` — a memo hit for indexed files).
///
/// The workspace-wide trait-alias fallback only runs when no candidate
/// actually declares `name` — i.e. it's not a class/function/method/
/// property/constant under that literal spelling anywhere. When `name` *is*
/// declared as something but that something has no `CallHierarchyItem`
/// (nothing currently classifies as one other than functions/methods/
/// class-likes), there is nothing left to try: a name that is already a
/// literal declaration is never also a trait-alias spelling.
pub fn prepare_call_hierarchy_indexed(
    name: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
    mention_candidates: &dyn Fn(&str) -> Vec<Uri>,
) -> Option<CallHierarchyItem> {
    for uri in &mention_candidates(name) {
        let Some(doc) = get_doc(uri) else { continue };
        if let Some(item) = find_declaration_item(name, &doc.program().stmts, doc.view(), uri) {
            return Some(item);
        }
    }
    // `name` might be a `use Trait { method as name; }` alias, which never
    // appears as a literal declaration anywhere.
    let original = resolve_trait_alias_indexed(name, wi)?;
    if original == name {
        return None;
    }
    prepare_call_hierarchy_indexed(&original, wi, get_doc, mention_candidates)
}

/// Resolves `name` against every class's recorded trait-method aliases in
/// the workspace index (`FileIndex::extract` already collects these from
/// `use Trait { method as alias; }`). Fallback only — the common path
/// resolves via a literal declaration, which never matches an alias spelling.
fn resolve_trait_alias_indexed(
    name: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
) -> Option<String> {
    let mut resolved = None;
    wi.for_each_class(|_, cls| {
        if resolved.is_some() {
            return;
        }
        for alias in &cls.trait_method_aliases {
            if alias.alias.as_ref() == name {
                resolved = Some(alias.original.to_string());
                break;
            }
        }
    });
    resolved
}

/// Calls made by the body of `item`, resolved by mir. Callee declarations are
/// located through mir and mapped back to hierarchy items by the name token
/// at the declaration.
pub fn outgoing_calls_via_mir(
    docs: &crate::document::document_store::DocumentStore,
    item: &CallHierarchyItem,
) -> Result<Vec<CallHierarchyOutgoingCall>, crate::document::document_store::ContentModified> {
    let Some(doc) = docs.get_doc_salsa(&item.uri) else {
        return Ok(Vec::new());
    };
    let source = doc.source();
    let offset = crate::text::position_to_byte_offset(source, item.selection_range.start) as u32;
    let callees = crate::navigation::mir_definition::mir_outgoing_callees(docs, &item.uri, offset)?;
    // mir attributes anonymous-class method bodies to the enclosing function.
    let anon_ranges = anonymous_class_ranges(&doc);

    let mut result: Vec<CallHierarchyOutgoingCall> = Vec::new();
    let mut index: HashMap<(Uri, Position), usize> = HashMap::new();
    for (callee, call_range) in callees {
        if anon_ranges
            .iter()
            .any(|r| range_contains(*r, call_range.start))
        {
            continue;
        }
        let Some(target_doc) = docs.get_doc_salsa(&callee.uri) else {
            continue;
        };
        let target_source = target_doc.source();
        let start = crate::text::position_to_byte_offset(target_source, callee.range.start);
        let end = crate::text::position_to_byte_offset(target_source, callee.range.end);
        let Some(name) = target_source.get(start..end) else {
            continue;
        };
        let Some(callee_item) = find_declaration_item(
            name,
            &target_doc.program().stmts,
            target_doc.view(),
            &callee.uri,
        ) else {
            continue;
        };
        let key = (callee_item.uri.clone(), callee_item.selection_range.start);
        if let Some(&idx) = index.get(&key) {
            // mir can report a call site more than once (e.g. loop update clauses).
            if !result[idx].from_ranges.contains(&call_range) {
                result[idx].from_ranges.push(call_range);
            }
        } else {
            index.insert(key, result.len());
            result.push(CallHierarchyOutgoingCall {
                to: callee_item,
                from_ranges: vec![call_range],
            });
        }
    }
    Ok(result)
}

fn anonymous_class_ranges(doc: &ParsedDoc) -> Vec<Range> {
    struct Finder<'a> {
        doc: &'a ParsedDoc,
        out: Vec<Range>,
    }
    impl<'arena, 'src> Visitor<'arena, 'src> for Finder<'_> {
        fn visit_expr(&mut self, expr: &php_ast::Expr<'arena, 'src>) -> ControlFlow<()> {
            if matches!(expr.kind, ExprKind::AnonymousClass(_)) {
                self.out.push(self.doc.view().range_of(expr.span));
            }
            walk_expr(self, expr)
        }
    }
    let mut finder = Finder {
        doc,
        out: Vec::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = finder.visit_stmt(stmt);
    }
    finder.out
}

/// Find all callers of `item` and return them grouped by enclosing function.
///
/// Call sites come from mir's reference posting lists (`meth:`/`methname:`/
/// `fn:` keys via `indexed_references`), resolved against the item's own
/// declaring document; only the documents that actually contain call sites
/// are parsed to find the enclosing caller.
pub fn incoming_calls_indexed(
    item: &CallHierarchyItem,
    store: &crate::document::document_store::DocumentStore,
    cancel_rev: Option<u64>,
) -> Vec<CallHierarchyIncomingCall> {
    let Some(item_doc) = store.get_doc_salsa(&item.uri) else {
        return Vec::new();
    };
    let imports = crate::navigation::references::collect_file_imports(&item_doc);
    let resolve = |name: &str| -> String {
        crate::navigation::moniker::resolve_fqn(&item_doc, name, &imports)
            .trim_start_matches('\\')
            .to_string()
    };
    // `detail` carries the declaring class-like's short name for methods; an
    // unresolvable owner becomes the empty class, which mir answers from its
    // name-keyed fallback postings.
    let symbol = if item.kind == SymbolKind::METHOD {
        let owner = item.detail.as_deref().map(&resolve).unwrap_or_default();
        mir_analyzer::Name::method(owner.as_str(), &item.name)
    } else {
        mir_analyzer::Name::function(resolve(&item.name))
    };

    let files = store.reference_candidate_files(&symbol);
    let mut call_sites: Vec<tower_lsp_server::ls_types::Location> = store
        .indexed_references(&symbol, &files, false, cancel_rev)
        .unwrap_or_default()
        .into_iter()
        .filter_map(crate::navigation::references::session_tuple_to_location)
        .collect();
    crate::navigation::references::dedup_ref_locations(&mut call_sites);

    let mut result: Vec<CallHierarchyIncomingCall> = Vec::new();
    // Track (caller_name, caller_uri) → index in `result` for O(1) dedup.
    let mut index: HashMap<(String, Uri), usize> = HashMap::new();
    // Parse only the documents call sites landed in, each at most once.
    let mut doc_cache: HashMap<Uri, Option<Arc<ParsedDoc>>> = HashMap::new();

    for loc in call_sites {
        let doc = doc_cache
            .entry(loc.uri.clone())
            .or_insert_with(|| store.get_doc_salsa(&loc.uri));
        let caller = doc.as_ref().and_then(|doc| {
            enclosing_function(doc.view(), &doc.program().stmts, loc.range.start, &loc.uri)
        });

        let key = if let Some(ref ci) = caller {
            (ci.name.clone(), ci.uri.clone())
        } else {
            ("<file scope>".to_string(), loc.uri.clone())
        };

        if let Some(&idx) = index.get(&key) {
            result[idx].from_ranges.push(loc.range);
        } else {
            let from = caller.unwrap_or_else(|| CallHierarchyItem {
                name: "<file scope>".to_string(),
                kind: SymbolKind::FILE,
                tags: None,
                detail: None,
                uri: loc.uri.clone(),
                range: loc.range,
                selection_range: loc.range,
                data: None,
            });
            let idx = result.len();
            index.insert(key, idx);
            result.push(CallHierarchyIncomingCall {
                from,
                from_ranges: vec![loc.range],
            });
        }
    }

    result
}

// === Internal helpers ===

fn find_declaration_item(
    name: &str,
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    uri: &Uri,
) -> Option<CallHierarchyItem> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == name => {
                let range = sv.range_of(stmt.span);
                let sel = sv.name_range_after_attrs(&f.name.to_string(), &f.attributes, stmt.span);
                return Some(CallHierarchyItem {
                    name: name.to_string(),
                    kind: SymbolKind::FUNCTION,
                    tags: None,
                    detail: None,
                    uri: uri.clone(),
                    range,
                    selection_range: sel,
                    data: None,
                });
            }
            // Class-name-itself match: `new X()` extracts the bare class name as
            // its "callee", and a class declaration named `X` is a valid
            // candidate here — without this arm, that lookup always missed
            // and fell through to the workspace-wide trait-alias scan for
            // every `new` expression in the codebase.
            StmtKind::Class(c) if c.name.is_some_and(|n| n == name) => {
                let range = sv.range_of(stmt.span);
                let sel = sv.name_range_after_attrs(name, &c.attributes, stmt.span);
                return Some(CallHierarchyItem {
                    name: name.to_string(),
                    kind: SymbolKind::CLASS,
                    tags: None,
                    detail: None,
                    uri: uri.clone(),
                    range,
                    selection_range: sel,
                    data: None,
                });
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == name
                    {
                        let range = sv.range_of(member.span);
                        let sel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        return Some(CallHierarchyItem {
                            name: name.to_string(),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            detail: c.name.map(|n| n.to_string()),
                            uri: uri.clone(),
                            range,
                            selection_range: sel,
                            data: None,
                        });
                    }
                }
            }
            StmtKind::Interface(i) if i.name == name => {
                let range = sv.range_of(stmt.span);
                let sel = sv.name_range_after_attrs(name, &i.attributes, stmt.span);
                return Some(CallHierarchyItem {
                    name: name.to_string(),
                    kind: SymbolKind::INTERFACE,
                    tags: None,
                    detail: None,
                    uri: uri.clone(),
                    range,
                    selection_range: sel,
                    data: None,
                });
            }
            StmtKind::Interface(i) => {
                for member in i.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == name
                    {
                        let range = sv.range_of(member.span);
                        let sel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        return Some(CallHierarchyItem {
                            name: name.to_string(),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            detail: Some(i.name.to_string()),
                            uri: uri.clone(),
                            range,
                            selection_range: sel,
                            data: None,
                        });
                    }
                }
            }
            StmtKind::Trait(t) if t.name == name => {
                let range = sv.range_of(stmt.span);
                let sel = sv.name_range_after_attrs(name, &t.attributes, stmt.span);
                return Some(CallHierarchyItem {
                    name: name.to_string(),
                    kind: SymbolKind::CLASS,
                    tags: None,
                    detail: None,
                    uri: uri.clone(),
                    range,
                    selection_range: sel,
                    data: None,
                });
            }
            StmtKind::Trait(t) => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == name
                    {
                        let range = sv.range_of(member.span);
                        let sel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        return Some(CallHierarchyItem {
                            name: name.to_string(),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            detail: Some(t.name.to_string()),
                            uri: uri.clone(),
                            range,
                            selection_range: sel,
                            data: None,
                        });
                    }
                }
            }
            StmtKind::Enum(e) if e.name == name => {
                let range = sv.range_of(stmt.span);
                let sel = sv.name_range_after_attrs(name, &e.attributes, stmt.span);
                return Some(CallHierarchyItem {
                    name: name.to_string(),
                    kind: SymbolKind::ENUM,
                    tags: None,
                    detail: None,
                    uri: uri.clone(),
                    range,
                    selection_range: sel,
                    data: None,
                });
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == name
                    {
                        let range = sv.range_of(member.span);
                        let sel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        return Some(CallHierarchyItem {
                            name: name.to_string(),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            detail: Some(e.name.to_string()),
                            uri: uri.clone(),
                            range,
                            selection_range: sel,
                            data: None,
                        });
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(item) = find_declaration_item(name, &inner.stmts, sv, uri)
                {
                    return Some(item);
                }
            }
            _ => {}
        }
    }
    // `name` may be a `use Trait { method as name; }` alias rather than a
    // literal declaration anywhere — retry under the trait method's real name.
    if let Some(original) = resolve_trait_alias(name, stmts)
        && original != name
    {
        return find_declaration_item(&original, stmts, sv, uri);
    }
    None
}

/// If `name` is introduced by `use Trait { method as name; }` on some class
/// in `stmts`, returns the trait method's real declared name. Without this,
/// a call site written under the alias (the only spelling that exists in
/// source) can never resolve — no AST node is literally named the alias.
fn resolve_trait_alias(name: &str, stmts: &[Stmt<'_, '_>]) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::TraitUse(tu) = &member.kind {
                        for adaptation in tu.adaptations.iter() {
                            if let TraitAdaptationKind::Alias {
                                method,
                                new_name: Some(new_name),
                                ..
                            } = &adaptation.kind
                                && new_name.to_string_repr() == name
                            {
                                return Some(method.to_string_repr().into_owned());
                            }
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(found) = resolve_trait_alias(name, &inner.stmts)
                {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

fn enclosing_function(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    pos: Position,
    uri: &Uri,
) -> Option<CallHierarchyItem> {
    for stmt in stmts {
        if let Some(item) = enclosing_in_stmt(sv, stmt, pos, uri) {
            return Some(item);
        }
    }
    None
}

fn enclosing_in_stmt(
    sv: SourceView<'_>,
    stmt: &Stmt<'_, '_>,
    pos: Position,
    uri: &Uri,
) -> Option<CallHierarchyItem> {
    let range = sv.range_of(stmt.span);
    if !range_contains(range, pos) {
        return None;
    }
    match &stmt.kind {
        StmtKind::Function(f) => {
            let sel = sv.name_range_after_attrs(&f.name.to_string(), &f.attributes, stmt.span);
            Some(CallHierarchyItem {
                name: f.name.to_string(),
                kind: SymbolKind::FUNCTION,
                tags: None,
                detail: None,
                uri: uri.clone(),
                range,
                selection_range: sel,
                data: None,
            })
        }
        StmtKind::Class(c) => {
            for member in c.body.members.iter() {
                let m_range = sv.range_of(member.span);
                if range_contains(m_range, pos)
                    && let ClassMemberKind::Method(m) = &member.kind
                {
                    let sel =
                        sv.name_range_after_attrs(&m.name.to_string(), &m.attributes, member.span);
                    return Some(CallHierarchyItem {
                        name: m.name.to_string(),
                        kind: SymbolKind::METHOD,
                        tags: None,
                        detail: c.name.map(|n| n.to_string()),
                        uri: uri.clone(),
                        range: m_range,
                        selection_range: sel,
                        data: None,
                    });
                }
            }
            None
        }
        StmtKind::Trait(t) => {
            for member in t.body.members.iter() {
                let m_range = sv.range_of(member.span);
                if range_contains(m_range, pos)
                    && let ClassMemberKind::Method(m) = &member.kind
                {
                    let sel =
                        sv.name_range_after_attrs(&m.name.to_string(), &m.attributes, member.span);
                    return Some(CallHierarchyItem {
                        name: m.name.to_string(),
                        kind: SymbolKind::METHOD,
                        tags: None,
                        detail: Some(t.name.to_string()),
                        uri: uri.clone(),
                        range: m_range,
                        selection_range: sel,
                        data: None,
                    });
                }
            }
            None
        }
        StmtKind::Enum(e) => {
            for member in e.body.members.iter() {
                let m_range = sv.range_of(member.span);
                if range_contains(m_range, pos)
                    && let EnumMemberKind::Method(m) = &member.kind
                {
                    let sel =
                        sv.name_range_after_attrs(&m.name.to_string(), &m.attributes, member.span);
                    return Some(CallHierarchyItem {
                        name: m.name.to_string(),
                        kind: SymbolKind::METHOD,
                        tags: None,
                        detail: Some(e.name.to_string()),
                        uri: uri.clone(),
                        range: m_range,
                        selection_range: sel,
                        data: None,
                    });
                }
            }
            None
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                return enclosing_function(sv, &inner.stmts, pos, uri);
            }
            None
        }
        _ => None,
    }
}

fn range_contains(range: Range, pos: Position) -> bool {
    if pos.line < range.start.line || pos.line > range.end.line {
        return false;
    }
    if pos.line == range.start.line && pos.character < range.start.character {
        return false;
    }
    if pos.line == range.end.line && pos.character >= range.end.character {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── range_contains boundary regression tests ─────────────────────────────
    //
    // These unit tests cover internal implementation details (helper function
    // boundary semantics). They are kept as unit tests rather than migrated to
    // E2E because they test infrastructure mechanics, not user-facing LSP
    // features. Protocol-wired E2E tests for call hierarchy features are in
    // tests/navigation/feature_hierarchy.rs and provide comprehensive coverage
    // of the public API.

    #[test]
    fn range_contains_excludes_exact_end_position() {
        // LSP ranges are half-open [start, end).  A position exactly at
        // range.end is OUTSIDE the range.  The old code used `>` instead of
        // `>=`, which incorrectly included the end position.
        let range = Range {
            start: Position {
                line: 1,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 5,
            },
        };
        // One past the last character on the end line — clearly outside.
        assert!(
            !range_contains(
                range,
                Position {
                    line: 3,
                    character: 6
                }
            ),
            "position after end must be outside"
        );
        // Exactly at end — outside per LSP half-open semantics.
        assert!(
            !range_contains(
                range,
                Position {
                    line: 3,
                    character: 5
                }
            ),
            "position exactly at range.end must be outside (half-open range)"
        );
        // One before end — inside.
        assert!(
            range_contains(
                range,
                Position {
                    line: 3,
                    character: 4
                }
            ),
            "position just before end must be inside"
        );
        // Start of range — inside.
        assert!(
            range_contains(
                range,
                Position {
                    line: 1,
                    character: 0
                }
            ),
            "start position must be inside"
        );
    }
}
