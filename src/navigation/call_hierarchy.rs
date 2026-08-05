use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use php_ast::visitor::{Visitor, walk_expr, walk_stmt};
use php_ast::{
    ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Span, Stmt, StmtKind,
    TraitAdaptationKind,
};
use tower_lsp_server::ls_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind, Uri,
};

use crate::document::ast::{ParsedDoc, SourceView, span_to_range};
use crate::lang::is_php_keyword;

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

/// Finds all calls made by the body of `item.name`, resolving the item's own
/// document and every callee declaration through the workspace aggregate
/// instead of a pre-materialised all-docs list. Avoids the per-callee
/// O(workspace) scan that made outgoing calls quadratic in practice.
pub fn outgoing_calls_indexed(
    item: &CallHierarchyItem,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
    mention_candidates: &dyn Fn(&str) -> Vec<Uri>,
) -> Vec<CallHierarchyOutgoingCall> {
    let Some(doc) = get_doc(&item.uri) else {
        return Vec::new();
    };
    let item_source = doc.source();
    let mut calls: Vec<(String, Span)> = Vec::new();
    collect_calls_for(&item.name, &doc.program().stmts, &mut calls);

    let mut result: Vec<CallHierarchyOutgoingCall> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let item_line_starts = doc.line_starts();
    for (callee_name, span) in calls {
        let call_range = span_to_range(item_source, item_line_starts, span);
        if let Some(&idx) = index.get(&callee_name) {
            result[idx].from_ranges.push(call_range);
        } else if let Some(callee_item) =
            prepare_call_hierarchy_indexed(&callee_name, wi, get_doc, mention_candidates)
        {
            let idx = result.len();
            index.insert(callee_name, idx);
            result.push(CallHierarchyOutgoingCall {
                to: callee_item,
                from_ranges: vec![call_range],
            });
        }
    }

    result
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

/// Collect all (callee_name, span) for calls made inside the body of `fn_name`.
fn collect_calls_for(fn_name: &str, stmts: &[Stmt<'_, '_>], out: &mut Vec<(String, Span)>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == fn_name => {
                calls_in_stmts(&f.body.stmts, out);
                return;
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == fn_name
                        && let Some(body) = &m.body
                    {
                        calls_in_stmts(&body.stmts, out);
                        return;
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == fn_name
                        && let Some(body) = &m.body
                    {
                        calls_in_stmts(&body.stmts, out);
                        return;
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == fn_name
                        && let Some(body) = &m.body
                    {
                        calls_in_stmts(&body.stmts, out);
                        return;
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_calls_for(fn_name, &inner.stmts, out);
                }
            }
            _ => {}
        }
    }
}

/// Collects all (callee_name, span) call sites reachable from a slice of statements,
/// without descending into nested named declarations (functions, classes, etc.).
fn calls_in_stmts(stmts: &[Stmt<'_, '_>], out: &mut Vec<(String, Span)>) {
    let mut collector = CallCollector { out };
    for stmt in stmts {
        let _ = collector.visit_stmt(stmt);
    }
}

struct CallCollector<'c> {
    out: &'c mut Vec<(String, Span)>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for CallCollector<'_> {
    fn visit_expr(&mut self, expr: &php_ast::Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            ExprKind::FunctionCall(f) => {
                if let ExprKind::Identifier(name) = &f.name.kind {
                    self.out.push((name.to_string(), f.name.span));
                }
            }
            ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
                if let ExprKind::Identifier(name) = &m.method.kind {
                    self.out.push((name.to_string(), m.method.span));
                }
            }
            ExprKind::StaticMethodCall(s) => {
                if let ExprKind::Identifier(name) = &s.method.kind {
                    self.out.push((name.to_string(), s.method.span));
                }
            }
            ExprKind::New(n) => {
                if let ExprKind::Identifier(class_name) = &n.class.kind {
                    let class_name = class_name.to_string();
                    // `self`/`static`/`parent` are late-binding class refs, not
                    // literal declarations — nothing ever declares them, which
                    // would otherwise trigger the workspace-wide trait-alias
                    // scan in `prepare_call_hierarchy_indexed` on every
                    // `new self/static/parent()` call site.
                    if !is_php_keyword(&class_name) {
                        self.out.push((class_name, n.class.span));
                    }
                }
            }
            // First-class callable syntax (PHP 8.1): `foo(...)`, `$obj->method(...)`,
            // `$obj?->method(...)`, `Foo::bar(...)` — same callee-name extraction as
            // the corresponding regular call, just without arguments.
            ExprKind::CallableCreate(cc) => match &cc.kind {
                php_ast::CallableCreateKind::Function(f) => {
                    if let ExprKind::Identifier(name) = &f.kind {
                        self.out.push((name.to_string(), f.span));
                    }
                }
                php_ast::CallableCreateKind::Method { method, .. }
                | php_ast::CallableCreateKind::NullsafeMethod { method, .. }
                | php_ast::CallableCreateKind::StaticMethod { method, .. } => {
                    if let ExprKind::Identifier(name) = &method.kind {
                        self.out.push((name.to_string(), method.span));
                    }
                }
            },
            // An anonymous class is its own callable unit (like a nested named
            // class); its method bodies are not outgoing calls of whatever
            // function/method textually contains the `new class {...}` expression.
            ExprKind::AnonymousClass(_) => return ControlFlow::Continue(()),
            _ => {}
        }
        walk_expr(self, expr)
    }

    fn visit_stmt(&mut self, stmt: &php_ast::Stmt<'arena, 'src>) -> ControlFlow<()> {
        // Skip nested named declarations — they are separate callable units with
        // their own call hierarchy entries; their internals are not outgoing calls
        // of the function currently being analysed.
        match &stmt.kind {
            StmtKind::Function(_)
            | StmtKind::Class(_)
            | StmtKind::Trait(_)
            | StmtKind::Enum(_)
            | StmtKind::Interface(_) => ControlFlow::Continue(()),
            _ => walk_stmt(self, stmt),
        }
    }
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
