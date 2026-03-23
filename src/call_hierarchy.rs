use std::sync::Arc;

use php_parser_rs::lexer::token::Span;
use php_parser_rs::parser::ast::{
    arguments::Argument,
    classes::ClassMember,
    identifiers::Identifier as AstIdentifier,
    namespaces::NamespaceStatement,
    Expression, Statement,
};
use tower_lsp::lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind, Url,
};

use crate::diagnostics::span_to_position;
use crate::references::find_references;

/// Find the declaration matching `name` and return a `CallHierarchyItem`.
pub fn prepare_call_hierarchy(
    name: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Option<CallHierarchyItem> {
    for (uri, ast) in all_docs {
        if let Some(item) = find_declaration_item(name, ast, uri) {
            return Some(item);
        }
    }
    None
}

/// Find all callers of `item.name` and return them grouped by enclosing function.
pub fn incoming_calls(
    item: &CallHierarchyItem,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Vec<CallHierarchyIncomingCall> {
    let call_sites = find_references(&item.name, all_docs, false);

    let mut result: Vec<CallHierarchyIncomingCall> = Vec::new();

    for loc in call_sites {
        // Find the enclosing function/method for this call site
        let ast = all_docs
            .iter()
            .find(|(u, _)| *u == loc.uri)
            .map(|(_, a)| a.as_slice())
            .unwrap_or(&[]);
        let caller = enclosing_function(ast, loc.range.start, &loc.uri);

        // Merge into existing entry if same caller, else add new
        if let Some(caller_item) = caller {
            if let Some(entry) = result.iter_mut().find(|e| {
                e.from.name == caller_item.name && e.from.uri == caller_item.uri
            }) {
                entry.from_ranges.push(loc.range);
            } else {
                result.push(CallHierarchyIncomingCall {
                    from: caller_item,
                    from_ranges: vec![loc.range],
                });
            }
        } else {
            // Call is at file scope — represent it with a synthetic item
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
            if let Some(entry) = result.iter_mut().find(|e| {
                e.from.name == synthetic.name && e.from.uri == synthetic.uri
            }) {
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
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Vec<CallHierarchyOutgoingCall> {
    // Find the function/method body
    let body_stmts = find_body(item, all_docs);

    // Collect all call targets (name → list of call-site spans)
    let mut calls: Vec<(String, Span)> = Vec::new();
    calls_in_stmts(&body_stmts, &mut calls);

    // For each unique callee name, find its declaration
    let mut result: Vec<CallHierarchyOutgoingCall> = Vec::new();
    for (callee_name, call_span) in calls {
        let call_pos = span_to_position(&call_span);
        let call_range = Range {
            start: call_pos,
            end: Position {
                line: call_pos.line,
                character: call_pos.character + callee_name.len() as u32,
            },
        };

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
    ast: &[Statement],
    uri: &Url,
) -> Option<CallHierarchyItem> {
    for stmt in ast {
        match stmt {
            Statement::Function(f) if f.name.value.to_string() == name => {
                let start = span_to_position(&f.name.span);
                let sel = Range {
                    start,
                    end: Position { line: start.line, character: start.character + name.len() as u32 },
                };
                let full_start = span_to_position(&f.function);
                let full_end = span_to_position(&f.body.right_brace);
                return Some(CallHierarchyItem {
                    name: name.to_string(),
                    kind: SymbolKind::FUNCTION,
                    tags: None,
                    detail: None,
                    uri: uri.clone(),
                    range: Range {
                        start: full_start,
                        end: Position { line: full_end.line, character: full_end.character + 1 },
                    },
                    selection_range: sel,
                    data: None,
                });
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    match member {
                        ClassMember::ConcreteMethod(m) if m.name.value.to_string() == name => {
                            let start = span_to_position(&m.name.span);
                            let sel = Range {
                                start,
                                end: Position { line: start.line, character: start.character + name.len() as u32 },
                            };
                            let full_start = span_to_position(&m.function);
                            let full_end = span_to_position(&m.body.right_brace);
                            return Some(CallHierarchyItem {
                                name: name.to_string(),
                                kind: SymbolKind::METHOD,
                                tags: None,
                                detail: Some(c.name.value.to_string()),
                                uri: uri.clone(),
                                range: Range {
                                    start: full_start,
                                    end: Position { line: full_end.line, character: full_end.character + 1 },
                                },
                                selection_range: sel,
                                data: None,
                            });
                        }
                        _ => {}
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(item) = find_declaration_item(name, inner, uri) {
                    return Some(item);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find the enclosing function or method for a given position in the AST.
fn enclosing_function(ast: &[Statement], pos: Position, uri: &Url) -> Option<CallHierarchyItem> {
    for stmt in ast {
        if let Some(item) = enclosing_in_stmt(stmt, pos, uri) {
            return Some(item);
        }
    }
    None
}

fn enclosing_in_stmt(stmt: &Statement, pos: Position, uri: &Url) -> Option<CallHierarchyItem> {
    match stmt {
        Statement::Function(f) => {
            let start_line = (f.function.line as u32).saturating_sub(1);
            let end_line = (f.body.right_brace.line as u32).saturating_sub(1);
            if pos.line < start_line || pos.line > end_line {
                return None;
            }
            let fname = f.name.value.to_string();
            let name_pos = span_to_position(&f.name.span);
            let sel = Range {
                start: name_pos,
                end: Position { line: name_pos.line, character: name_pos.character + fname.len() as u32 },
            };
            let full_start = span_to_position(&f.function);
            let full_end = span_to_position(&f.body.right_brace);
            Some(CallHierarchyItem {
                name: fname,
                kind: SymbolKind::FUNCTION,
                tags: None,
                detail: None,
                uri: uri.clone(),
                range: Range {
                    start: full_start,
                    end: Position { line: full_end.line, character: full_end.character + 1 },
                },
                selection_range: sel,
                data: None,
            })
        }
        Statement::Class(c) => {
            let class_start = (c.class.line as u32).saturating_sub(1);
            let class_end = (c.body.right_brace.line as u32).saturating_sub(1);
            if pos.line < class_start || pos.line > class_end {
                return None;
            }
            for member in &c.body.members {
                if let ClassMember::ConcreteMethod(m) = member {
                    let mstart = (m.function.line as u32).saturating_sub(1);
                    let mend = (m.body.right_brace.line as u32).saturating_sub(1);
                    if pos.line >= mstart && pos.line <= mend {
                        let mname = m.name.value.to_string();
                        let name_pos = span_to_position(&m.name.span);
                        let sel = Range {
                            start: name_pos,
                            end: Position { line: name_pos.line, character: name_pos.character + mname.len() as u32 },
                        };
                        let full_start = span_to_position(&m.function);
                        let full_end = span_to_position(&m.body.right_brace);
                        return Some(CallHierarchyItem {
                            name: mname,
                            kind: SymbolKind::METHOD,
                            tags: None,
                            detail: Some(c.name.value.to_string()),
                            uri: uri.clone(),
                            range: Range {
                                start: full_start,
                                end: Position { line: full_end.line, character: full_end.character + 1 },
                            },
                            selection_range: sel,
                            data: None,
                        });
                    }
                }
            }
            None
        }
        Statement::Namespace(ns) => {
            let inner = match ns {
                NamespaceStatement::Unbraced(u) => &u.statements[..],
                NamespaceStatement::Braced(b) => &b.body.statements[..],
            };
            for s in inner {
                if let Some(item) = enclosing_in_stmt(s, pos, uri) {
                    return Some(item);
                }
            }
            None
        }
        _ => None,
    }
}

/// Find the body statements for the named function/method in item.
fn find_body(
    item: &CallHierarchyItem,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Vec<Statement> {
    for (uri, ast) in all_docs {
        if *uri == item.uri {
            if let Some(stmts) = body_stmts_for(item, ast) {
                return stmts;
            }
        }
    }
    vec![]
}

fn body_stmts_for(item: &CallHierarchyItem, ast: &[Statement]) -> Option<Vec<Statement>> {
    for stmt in ast {
        match stmt {
            Statement::Function(f) if f.name.value.to_string() == item.name => {
                return Some(f.body.statements.clone());
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    if let ClassMember::ConcreteMethod(m) = member {
                        if m.name.value.to_string() == item.name {
                            return Some(m.body.statements.clone());
                        }
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                if let Some(stmts) = body_stmts_for(item, inner) {
                    return Some(stmts);
                }
            }
            _ => {}
        }
    }
    None
}

/// Walk statements collecting (callee_name, call_span) for all function and method calls.
fn calls_in_stmts(stmts: &[Statement], out: &mut Vec<(String, Span)>) {
    for stmt in stmts {
        calls_in_stmt(stmt, out);
    }
}

fn calls_in_stmt(stmt: &Statement, out: &mut Vec<(String, Span)>) {
    match stmt {
        Statement::Expression(e) => calls_in_expr(&e.expression, out),
        Statement::Return(r) => {
            if let Some(v) = &r.value {
                calls_in_expr(v, out);
            }
        }
        Statement::Echo(e) => {
            for expr in &e.values {
                calls_in_expr(expr, out);
            }
        }
        Statement::If(i) => {
            use php_parser_rs::parser::ast::control_flow::IfStatementBody;
            calls_in_expr(&i.condition, out);
            match &i.body {
                IfStatementBody::Statement { statement, elseifs, r#else } => {
                    calls_in_stmt(statement, out);
                    for ei in elseifs {
                        calls_in_expr(&ei.condition, out);
                        calls_in_stmt(&ei.statement, out);
                    }
                    if let Some(e) = r#else {
                        calls_in_stmt(&e.statement, out);
                    }
                }
                IfStatementBody::Block { statements, elseifs, r#else, .. } => {
                    calls_in_stmts(statements, out);
                    for ei in elseifs {
                        calls_in_expr(&ei.condition, out);
                        calls_in_stmts(&ei.statements, out);
                    }
                    if let Some(e) = r#else {
                        calls_in_stmts(&e.statements, out);
                    }
                }
            }
        }
        Statement::While(w) => {
            use php_parser_rs::parser::ast::loops::WhileStatementBody;
            calls_in_expr(&w.condition, out);
            match &w.body {
                WhileStatementBody::Statement { statement } => calls_in_stmt(statement, out),
                WhileStatementBody::Block { statements, .. } => calls_in_stmts(statements, out),
            }
        }
        Statement::Foreach(f) => {
            use php_parser_rs::parser::ast::loops::{ForeachStatementBody, ForeachStatementIterator};
            let expr = match &f.iterator {
                ForeachStatementIterator::Value { expression, .. } => expression,
                ForeachStatementIterator::KeyAndValue { expression, .. } => expression,
            };
            calls_in_expr(expr, out);
            match &f.body {
                ForeachStatementBody::Statement { statement } => calls_in_stmt(statement, out),
                ForeachStatementBody::Block { statements, .. } => calls_in_stmts(statements, out),
            }
        }
        Statement::Try(t) => {
            calls_in_stmts(&t.body, out);
            for catch in &t.catches {
                calls_in_stmts(&catch.body, out);
            }
            if let Some(finally) = &t.finally {
                calls_in_stmts(&finally.body, out);
            }
        }
        Statement::Block(b) => calls_in_stmts(&b.statements, out),
        _ => {}
    }
}

fn calls_in_expr(expr: &Expression, out: &mut Vec<(String, Span)>) {
    match expr {
        Expression::FunctionCall(f) => {
            if let Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) = f.target.as_ref() {
                out.push((si.value.to_string(), si.span));
            } else {
                calls_in_expr(&f.target, out);
            }
            call_args(&f.arguments, out);
        }
        Expression::MethodCall(m) => {
            calls_in_expr(&m.target, out);
            if let Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) = m.method.as_ref() {
                out.push((si.value.to_string(), si.span));
            }
            call_args(&m.arguments, out);
        }
        Expression::NullsafeMethodCall(m) => {
            calls_in_expr(&m.target, out);
            if let Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) = m.method.as_ref() {
                out.push((si.value.to_string(), si.span));
            }
            call_args(&m.arguments, out);
        }
        Expression::StaticMethodCall(s) => {
            calls_in_expr(&s.target, out);
            call_args(&s.arguments, out);
        }
        Expression::AssignmentOperation(a) => {
            calls_in_expr(a.left(), out);
            calls_in_expr(a.right(), out);
        }
        Expression::Ternary(t) => {
            calls_in_expr(&t.condition, out);
            calls_in_expr(&t.then, out);
            calls_in_expr(&t.r#else, out);
        }
        Expression::Coalesce(c) => {
            calls_in_expr(&c.lhs, out);
            calls_in_expr(&c.rhs, out);
        }
        Expression::Parenthesized(p) => calls_in_expr(&p.expr, out),
        Expression::Concat(c) => {
            calls_in_expr(&c.left, out);
            calls_in_expr(&c.right, out);
        }
        Expression::Closure(c) => calls_in_stmts(&c.body.statements, out),
        Expression::ArrowFunction(a) => calls_in_expr(&a.body, out),
        _ => {}
    }
}

fn call_args(args: &php_parser_rs::parser::ast::arguments::ArgumentList, out: &mut Vec<(String, Span)>) {
    for arg in &args.arguments {
        match arg {
            Argument::Positional(p) => calls_in_expr(&p.value, out),
            Argument::Named(n) => calls_in_expr(&n.value, out),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ast(source: &str) -> Vec<Statement> {
        match php_parser_rs::parser::parse(source) {
            Ok(ast) => ast,
            Err(stack) => stack.partial,
        }
    }

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, source: &str) -> (Url, Arc<Vec<Statement>>) {
        (uri(path), Arc::new(parse_ast(source)))
    }

    #[test]
    fn prepare_finds_function_declaration() {
        let docs = vec![doc("/a.php", "<?php\nfunction greet() {}") ];
        let item = prepare_call_hierarchy("greet", &docs);
        assert!(item.is_some(), "should find greet");
        let item = item.unwrap();
        assert_eq!(item.name, "greet");
        assert_eq!(item.kind, SymbolKind::FUNCTION);
    }

    #[test]
    fn prepare_finds_method_declaration() {
        let docs = vec![doc("/a.php", "<?php\nclass Foo { public function run() {} }")];
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
        let docs: Vec<(Url, Arc<Vec<Statement>>)> = vec![];
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
        assert!(incoming.iter().any(|c| c.from.name == "main"), "main should be a caller");
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
        assert!(outgoing.iter().any(|c| c.to.name == "helper"), "helper should be a callee");
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
        assert!(outgoing.iter().any(|c| c.to.name == "helper"), "cross-file callee not found");
    }

    #[test]
    fn incoming_calls_cross_file() {
        let a = doc("/a.php", "<?php\nfunction greet() {}");
        let b = doc("/b.php", "<?php\nfunction run() { greet(); }");
        let docs = vec![a, b];
        let item = prepare_call_hierarchy("greet", &docs).unwrap();
        let incoming = incoming_calls(&item, &docs);
        assert!(incoming.iter().any(|c| c.from.name == "run"), "cross-file caller not found");
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
        assert_eq!(helper_entries.len(), 1, "helper should appear once (with two from_ranges)");
        assert_eq!(helper_entries[0].from_ranges.len(), 2, "should have two call-site ranges");
    }
}
