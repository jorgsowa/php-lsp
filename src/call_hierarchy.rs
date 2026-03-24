use std::sync::Arc;

use php_ast::{ClassMemberKind, ExprKind, NamespaceBody, Span, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind, Url,
};

use crate::ast::{ParsedDoc, name_range, span_to_range};
use crate::references::find_references;

/// Find the declaration matching `name` and return a `CallHierarchyItem`.
pub fn prepare_call_hierarchy(
    name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Option<CallHierarchyItem> {
    for (uri, doc) in all_docs {
        let source = doc.source();
        if let Some(item) = find_declaration_item(name, &doc.program().stmts, source, uri) {
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
    let call_sites = find_references(&item.name, all_docs, false);
    let mut result: Vec<CallHierarchyIncomingCall> = Vec::new();

    for loc in call_sites {
        let caller = all_docs
            .iter()
            .find(|(u, _)| *u == loc.uri)
            .and_then(|(_, doc)| {
                enclosing_function(
                    doc.source(),
                    &doc.program().stmts,
                    loc.range.start,
                    &loc.uri,
                )
            });

        if let Some(caller_item) = caller {
            if let Some(entry) = result
                .iter_mut()
                .find(|e| e.from.name == caller_item.name && e.from.uri == caller_item.uri)
            {
                entry.from_ranges.push(loc.range);
            } else {
                result.push(CallHierarchyIncomingCall {
                    from: caller_item,
                    from_ranges: vec![loc.range],
                });
            }
        } else {
            let synthetic = CallHierarchyItem {
                name: "<file scope>".to_string(),
                kind: SymbolKind::FILE,
                tags: None,
                detail: None,
                uri: loc.uri.clone(),
                range: loc.range,
                selection_range: loc.range,
                data: None,
            };
            if let Some(entry) = result
                .iter_mut()
                .find(|e| e.from.name == synthetic.name && e.from.uri == synthetic.uri)
            {
                entry.from_ranges.push(loc.range);
            } else {
                result.push(CallHierarchyIncomingCall {
                    from: synthetic,
                    from_ranges: vec![loc.range],
                });
            }
        }
    }

    result
}

/// Find all calls made by the body of `item.name`.
pub fn outgoing_calls(
    item: &CallHierarchyItem,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<CallHierarchyOutgoingCall> {
    let mut calls: Vec<(String, Span)> = Vec::new();
    let mut item_source = String::new();

    for (uri, doc) in all_docs {
        if *uri == item.uri {
            item_source = doc.source().to_string();
            collect_calls_for(&item.name, &doc.program().stmts, &mut calls);
            break;
        }
    }

    let mut result: Vec<CallHierarchyOutgoingCall> = Vec::new();
    for (callee_name, span) in calls {
        let call_range = span_to_range(&item_source, span);
        if let Some(existing) = result.iter_mut().find(|e| e.to.name == callee_name) {
            existing.from_ranges.push(call_range);
        } else if let Some(callee_item) = prepare_call_hierarchy(&callee_name, all_docs) {
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
    source: &str,
    uri: &Url,
) -> Option<CallHierarchyItem> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == name => {
                let range = span_to_range(source, stmt.span);
                let sel = name_range(source, f.name);
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
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == name {
                            let range = span_to_range(source, member.span);
                            let sel = name_range(source, m.name);
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
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(item) = find_declaration_item(name, inner, source, uri) {
                        return Some(item);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn enclosing_function(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    pos: Position,
    uri: &Url,
) -> Option<CallHierarchyItem> {
    for stmt in stmts {
        if let Some(item) = enclosing_in_stmt(source, stmt, pos, uri) {
            return Some(item);
        }
    }
    None
}

fn enclosing_in_stmt(
    source: &str,
    stmt: &Stmt<'_, '_>,
    pos: Position,
    uri: &Url,
) -> Option<CallHierarchyItem> {
    let range = span_to_range(source, stmt.span);
    if !range_contains(range, pos) {
        return None;
    }
    match &stmt.kind {
        StmtKind::Function(f) => {
            let sel = name_range(source, f.name);
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
                let m_range = span_to_range(source, member.span);
                if range_contains(m_range, pos) {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let sel = name_range(source, m.name);
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
            }
            None
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                return enclosing_function(source, inner, pos, uri);
            }
            None
        }
        _ => None,
    }
}

fn range_contains(range: Range, pos: Position) -> bool {
    pos.line >= range.start.line && pos.line <= range.end.line
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
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == fn_name {
                            if let Some(body) = &m.body {
                                calls_in_stmts(body, out);
                                return;
                            }
                        }
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

fn calls_in_stmts(stmts: &[Stmt<'_, '_>], out: &mut Vec<(String, Span)>) {
    for stmt in stmts {
        calls_in_stmt(stmt, out);
    }
}

fn calls_in_stmt(stmt: &Stmt<'_, '_>, out: &mut Vec<(String, Span)>) {
    match &stmt.kind {
        StmtKind::Expression(e) => calls_in_expr(e, out),
        StmtKind::Return(r) => {
            if let Some(v) = r {
                calls_in_expr(v, out);
            }
        }
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                calls_in_expr(expr, out);
            }
        }
        StmtKind::If(i) => {
            calls_in_expr(&i.condition, out);
            calls_in_stmt(i.then_branch, out);
            for ei in i.elseif_branches.iter() {
                calls_in_expr(&ei.condition, out);
                calls_in_stmt(&ei.body, out);
            }
            if let Some(e) = &i.else_branch {
                calls_in_stmt(e, out);
            }
        }
        StmtKind::While(w) => {
            calls_in_expr(&w.condition, out);
            calls_in_stmt(w.body, out);
        }
        StmtKind::For(f) => {
            for cond in f.condition.iter() {
                calls_in_expr(cond, out);
            }
            calls_in_stmt(f.body, out);
        }
        StmtKind::Foreach(f) => {
            calls_in_expr(&f.expr, out);
            calls_in_stmt(f.body, out);
        }
        StmtKind::TryCatch(t) => {
            calls_in_stmts(&t.body, out);
            for catch in t.catches.iter() {
                calls_in_stmts(&catch.body, out);
            }
            if let Some(finally) = &t.finally {
                calls_in_stmts(finally, out);
            }
        }
        StmtKind::Block(stmts) => calls_in_stmts(stmts, out),
        _ => {}
    }
}

fn calls_in_expr(expr: &php_ast::Expr<'_, '_>, out: &mut Vec<(String, Span)>) {
    match &expr.kind {
        ExprKind::FunctionCall(f) => {
            if let ExprKind::Identifier(name) = &f.name.kind {
                out.push((name.as_ref().to_string(), f.name.span));
            } else {
                calls_in_expr(f.name, out);
            }
            for arg in f.args.iter() {
                calls_in_expr(&arg.value, out);
            }
        }
        ExprKind::MethodCall(m) => {
            calls_in_expr(m.object, out);
            if let ExprKind::Identifier(name) = &m.method.kind {
                out.push((name.as_ref().to_string(), m.method.span));
            }
            for arg in m.args.iter() {
                calls_in_expr(&arg.value, out);
            }
        }
        ExprKind::NullsafeMethodCall(m) => {
            calls_in_expr(m.object, out);
            if let ExprKind::Identifier(name) = &m.method.kind {
                out.push((name.as_ref().to_string(), m.method.span));
            }
            for arg in m.args.iter() {
                calls_in_expr(&arg.value, out);
            }
        }
        ExprKind::StaticMethodCall(s) => {
            calls_in_expr(s.class, out);
            for arg in s.args.iter() {
                calls_in_expr(&arg.value, out);
            }
        }
        ExprKind::Assign(a) => {
            calls_in_expr(a.target, out);
            calls_in_expr(a.value, out);
        }
        ExprKind::Ternary(t) => {
            calls_in_expr(t.condition, out);
            if let Some(then_expr) = t.then_expr {
                calls_in_expr(then_expr, out);
            }
            calls_in_expr(t.else_expr, out);
        }
        ExprKind::NullCoalesce(n) => {
            calls_in_expr(n.left, out);
            calls_in_expr(n.right, out);
        }
        ExprKind::Binary(b) => {
            calls_in_expr(b.left, out);
            calls_in_expr(b.right, out);
        }
        ExprKind::Parenthesized(e) => calls_in_expr(e, out),
        _ => {}
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

    #[test]
    fn prepare_finds_function_declaration() {
        let docs = vec![doc("/a.php", "<?php\nfunction greet() {}")];
        let item = prepare_call_hierarchy("greet", &docs);
        assert!(item.is_some(), "should find greet");
        let item = item.unwrap();
        assert_eq!(item.name, "greet");
        assert_eq!(item.kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn prepare_finds_method_declaration() {
        let docs = vec![doc(
            "/a.php",
            "<?php\nclass Foo { public function run() {} }",
        )];
        let item = prepare_call_hierarchy("run", &docs);
        assert!(item.is_some(), "should find run");
        let item = item.unwrap();
        assert_eq!(item.name, "run");
        assert_eq!(item.kind, SymbolKind::METHOD);
    }

    #[test]
    fn prepare_returns_none_for_unknown() {
        let docs = vec![doc("/a.php", "<?php\nfunction greet() {}")];
        assert!(prepare_call_hierarchy("nonexistent", &docs).is_none());
    }

    #[test]
    fn prepare_returns_none_for_empty_docs() {
        let docs: Vec<(Url, Arc<ParsedDoc>)> = vec![];
        assert!(prepare_call_hierarchy("anything", &docs).is_none());
    }

    #[test]
    fn incoming_calls_finds_callers() {
        let docs = vec![doc(
            "/a.php",
            "<?php\nfunction greet() {}\nfunction main() { greet(); }",
        )];
        let item = prepare_call_hierarchy("greet", &docs).unwrap();
        let incoming = incoming_calls(&item, &docs);
        assert!(!incoming.is_empty(), "should find at least one caller");
        assert!(
            incoming.iter().any(|c| c.from.name == "main"),
            "main should be a caller"
        );
    }

    #[test]
    fn incoming_calls_empty_when_no_callers() {
        let docs = vec![doc("/a.php", "<?php\nfunction unused() {}")];
        let item = prepare_call_hierarchy("unused", &docs).unwrap();
        let incoming = incoming_calls(&item, &docs);
        assert!(incoming.is_empty(), "no callers expected");
    }

    #[test]
    fn outgoing_calls_finds_callees() {
        let docs = vec![doc(
            "/a.php",
            "<?php\nfunction helper() {}\nfunction main() { helper(); }",
        )];
        let item = prepare_call_hierarchy("main", &docs).unwrap();
        let outgoing = outgoing_calls(&item, &docs);
        assert!(!outgoing.is_empty(), "should find at least one callee");
        assert!(
            outgoing.iter().any(|c| c.to.name == "helper"),
            "helper should be a callee"
        );
    }

    #[test]
    fn outgoing_calls_empty_for_function_with_no_calls() {
        let docs = vec![doc("/a.php", "<?php\nfunction noop() { $x = 1; }")];
        let item = prepare_call_hierarchy("noop", &docs).unwrap();
        let outgoing = outgoing_calls(&item, &docs);
        assert!(outgoing.is_empty(), "no outgoing calls expected");
    }

    #[test]
    fn outgoing_calls_cross_file() {
        let a = doc("/a.php", "<?php\nfunction helper() {}");
        let b = doc("/b.php", "<?php\nfunction main() { helper(); }");
        let docs = vec![a, b];
        let item = prepare_call_hierarchy("main", &docs).unwrap();
        let outgoing = outgoing_calls(&item, &docs);
        assert!(
            outgoing.iter().any(|c| c.to.name == "helper"),
            "cross-file callee not found"
        );
    }

    #[test]
    fn incoming_calls_cross_file() {
        let a = doc("/a.php", "<?php\nfunction greet() {}");
        let b = doc("/b.php", "<?php\nfunction run() { greet(); }");
        let docs = vec![a, b];
        let item = prepare_call_hierarchy("greet", &docs).unwrap();
        let incoming = incoming_calls(&item, &docs);
        assert!(
            incoming.iter().any(|c| c.from.name == "run"),
            "cross-file caller not found"
        );
    }

    #[test]
    fn outgoing_calls_deduplicates_same_callee() {
        let docs = vec![doc(
            "/a.php",
            "<?php\nfunction helper() {}\nfunction main() { helper(); helper(); }",
        )];
        let item = prepare_call_hierarchy("main", &docs).unwrap();
        let outgoing = outgoing_calls(&item, &docs);
        let helper_entries: Vec<_> = outgoing.iter().filter(|c| c.to.name == "helper").collect();
        assert_eq!(
            helper_entries.len(),
            1,
            "helper should appear once (with two from_ranges)"
        );
        assert_eq!(
            helper_entries[0].from_ranges.len(),
            2,
            "should have two call-site ranges"
        );
    }
}
