use std::collections::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;

use php_ast::visitor::{Visitor, walk_expr, walk_stmt};
use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Span, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind, Url,
};

use crate::ast::{ParsedDoc, SourceView, span_to_range};
use crate::references::find_references;

/// Find the declaration matching `name` and return a `CallHierarchyItem`.
pub fn prepare_call_hierarchy(
    name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Option<CallHierarchyItem> {
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(item) = find_declaration_item(name, &doc.program().stmts, sv, uri) {
            return Some(item);
        }
    }
    None
}

/// Find all callers of `item.name` and return them grouped by enclosing function.
pub fn incoming_calls(
    item: &CallHierarchyItem,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<CallHierarchyIncomingCall> {
    let call_sites = find_references(&item.name, all_docs, false, None);
    // Build O(1) URI → doc map to avoid scanning all_docs for each call site.
    let doc_map: HashMap<&Url, &Arc<ParsedDoc>> = all_docs.iter().map(|(u, d)| (u, d)).collect();
    let mut result: Vec<CallHierarchyIncomingCall> = Vec::new();
    // Track (caller_name, caller_uri) → index in `result` for O(1) dedup.
    let mut index: HashMap<(String, Url), usize> = HashMap::new();

    for loc in call_sites {
        let caller = doc_map.get(&loc.uri).and_then(|doc| {
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

/// Find all calls made by the body of `item.name`.
pub fn outgoing_calls(
    item: &CallHierarchyItem,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<CallHierarchyOutgoingCall> {
    let Some((_, doc)) = all_docs.iter().find(|(uri, _)| *uri == item.uri) else {
        return Vec::new();
    };
    // Borrow sv.source() directly from the Arc to avoid cloning the whole file.
    let item_source = doc.source();
    let mut calls: Vec<(String, Span)> = Vec::new();
    collect_calls_for(&item.name, &doc.program().stmts, &mut calls);

    let mut result: Vec<CallHierarchyOutgoingCall> = Vec::new();
    // Track callee_name → index in `result` for O(1) dedup.
    let mut index: HashMap<String, usize> = HashMap::new();
    let item_line_starts = doc.line_starts();
    for (callee_name, span) in calls {
        let call_range = span_to_range(item_source, item_line_starts, span);
        if let Some(&idx) = index.get(&callee_name) {
            result[idx].from_ranges.push(call_range);
        } else if let Some(callee_item) = prepare_call_hierarchy(&callee_name, all_docs) {
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

// === Internal helpers ===

fn find_declaration_item(
    name: &str,
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    uri: &Url,
) -> Option<CallHierarchyItem> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == name => {
                let range = sv.range_of(stmt.span);
                let sel = sv.name_range(f.name);
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
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == name
                    {
                        let range = sv.range_of(member.span);
                        let sel = sv.name_range(m.name);
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
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == name
                    {
                        let range = sv.range_of(member.span);
                        let sel = sv.name_range(m.name);
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
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == name
                    {
                        let range = sv.range_of(member.span);
                        let sel = sv.name_range(m.name);
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
                    && let Some(item) = find_declaration_item(name, inner, sv, uri)
                {
                    return Some(item);
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
    uri: &Url,
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
    uri: &Url,
) -> Option<CallHierarchyItem> {
    let range = sv.range_of(stmt.span);
    if !range_contains(range, pos) {
        return None;
    }
    match &stmt.kind {
        StmtKind::Function(f) => {
            let sel = sv.name_range(f.name);
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
            for member in c.members.iter() {
                let m_range = sv.range_of(member.span);
                if range_contains(m_range, pos)
                    && let ClassMemberKind::Method(m) = &member.kind
                {
                    let sel = sv.name_range(m.name);
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
            for member in t.members.iter() {
                let m_range = sv.range_of(member.span);
                if range_contains(m_range, pos)
                    && let ClassMemberKind::Method(m) = &member.kind
                {
                    let sel = sv.name_range(m.name);
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
            for member in e.members.iter() {
                let m_range = sv.range_of(member.span);
                if range_contains(m_range, pos)
                    && let EnumMemberKind::Method(m) = &member.kind
                {
                    let sel = sv.name_range(m.name);
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
                return enclosing_function(sv, inner, pos, uri);
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
                calls_in_stmts(&f.body, out);
                return;
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == fn_name
                        && let Some(body) = &m.body
                    {
                        calls_in_stmts(body, out);
                        return;
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == fn_name
                        && let Some(body) = &m.body
                    {
                        calls_in_stmts(body, out);
                        return;
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == fn_name
                        && let Some(body) = &m.body
                    {
                        calls_in_stmts(body, out);
                        return;
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_calls_for(fn_name, inner, out);
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
