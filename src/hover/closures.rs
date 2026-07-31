use php_ast::{ClassMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::Position;

use crate::document::ast::{ParsedDoc, format_type_hint};

use super::formatting::format_params;

pub(crate) fn closure_hover(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    word: &str,
) -> Option<String> {
    let line_starts = doc.line_starts();
    let line = position.line as usize;
    let line_start = *line_starts.get(line)? as usize;
    let col_byte =
        crate::text::utf16_offset_to_byte(&source[line_start..], position.character as usize);
    let cursor_byte = (line_start + col_byte) as u32;

    find_closure_in_stmts(source, &doc.program().stmts, cursor_byte, word.len() as u32)
}

/// Recursively walk statements and their expressions looking for a closure
/// or arrow function whose span starts within `[cursor_byte, cursor_byte + word_len]`.
fn find_closure_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    cursor_byte: u32,
    word_len: u32,
) -> Option<String> {
    for stmt in stmts {
        if let Some(sig) = find_closure_in_stmt(source, stmt, cursor_byte, word_len) {
            return Some(sig);
        }
    }
    None
}

fn find_closure_in_stmt(
    source: &str,
    stmt: &Stmt<'_, '_>,
    cursor_byte: u32,
    word_len: u32,
) -> Option<String> {
    // Quick span filter: skip statements that don't contain the cursor.
    if stmt.span.end < cursor_byte || stmt.span.start > cursor_byte + word_len {
        return None;
    }
    match &stmt.kind {
        StmtKind::Expression(expr) | StmtKind::Throw(expr) => {
            find_closure_in_expr(source, expr, cursor_byte, word_len)
        }
        StmtKind::Return(Some(expr)) => find_closure_in_expr(source, expr, cursor_byte, word_len),
        StmtKind::Function(f) => {
            find_closure_in_stmts(source, &f.body.stmts, cursor_byte, word_len)
        }
        StmtKind::Class(c) => {
            for member in c.body.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                    && let Some(sig) =
                        find_closure_in_stmts(source, &body.stmts, cursor_byte, word_len)
                {
                    return Some(sig);
                }
            }
            None
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                find_closure_in_stmts(source, &inner.stmts, cursor_byte, word_len)
            } else {
                None
            }
        }
        StmtKind::Block(inner) => {
            find_closure_in_stmts(source, &inner.stmts, cursor_byte, word_len)
        }
        StmtKind::If(i) => {
            if let Some(sig) = find_closure_in_expr(source, &i.condition, cursor_byte, word_len)
                .or_else(|| find_closure_in_stmt(source, i.then_branch, cursor_byte, word_len))
            {
                return Some(sig);
            }
            for ei in i.elseif_branches.iter() {
                if let Some(sig) =
                    find_closure_in_expr(source, &ei.condition, cursor_byte, word_len)
                        .or_else(|| find_closure_in_stmt(source, &ei.body, cursor_byte, word_len))
                {
                    return Some(sig);
                }
            }
            if let Some(e) = &i.else_branch {
                find_closure_in_stmt(source, e, cursor_byte, word_len)
            } else {
                None
            }
        }
        StmtKind::While(w) => find_closure_in_expr(source, &w.condition, cursor_byte, word_len)
            .or_else(|| find_closure_in_stmt(source, w.body, cursor_byte, word_len)),
        StmtKind::DoWhile(d) => find_closure_in_stmt(source, d.body, cursor_byte, word_len)
            .or_else(|| find_closure_in_expr(source, &d.condition, cursor_byte, word_len)),
        StmtKind::For(f) => {
            for e in f.init.iter() {
                if let Some(sig) = find_closure_in_expr(source, e, cursor_byte, word_len) {
                    return Some(sig);
                }
            }
            for e in f.condition.iter() {
                if let Some(sig) = find_closure_in_expr(source, e, cursor_byte, word_len) {
                    return Some(sig);
                }
            }
            for e in f.update.iter() {
                if let Some(sig) = find_closure_in_expr(source, e, cursor_byte, word_len) {
                    return Some(sig);
                }
            }
            find_closure_in_stmt(source, f.body, cursor_byte, word_len)
        }
        StmtKind::Foreach(f) => find_closure_in_expr(source, &f.expr, cursor_byte, word_len)
            .or_else(|| find_closure_in_stmt(source, f.body, cursor_byte, word_len)),
        StmtKind::TryCatch(t) => {
            if let Some(sig) = find_closure_in_stmts(source, &t.body.stmts, cursor_byte, word_len) {
                return Some(sig);
            }
            for catch in t.catches.iter() {
                if let Some(sig) =
                    find_closure_in_stmts(source, &catch.body.stmts, cursor_byte, word_len)
                {
                    return Some(sig);
                }
            }
            if let Some(finally) = &t.finally {
                find_closure_in_stmts(source, &finally.stmts, cursor_byte, word_len)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[allow(clippy::only_used_in_recursion)]
fn find_closure_in_expr(
    source: &str,
    expr: &php_ast::Expr<'_, '_>,
    cursor_byte: u32,
    word_len: u32,
) -> Option<String> {
    if expr.span.end < cursor_byte || expr.span.start > cursor_byte + word_len {
        return None;
    }
    match &expr.kind {
        ExprKind::Closure(c) if c_span_matches(expr.span.start, cursor_byte, word_len) => {
            let params = format_params(&c.params);
            let ret = c
                .return_type
                .as_ref()
                .map(|r| format!(": {}", format_type_hint(r)))
                .unwrap_or_default();
            let static_kw = if c.is_static { "static " } else { "" };
            Some(format!("{}function({}){}", static_kw, params, ret))
        }
        ExprKind::ArrowFunction(af) if c_span_matches(expr.span.start, cursor_byte, word_len) => {
            let params = format_params(&af.params);
            let ret = af
                .return_type
                .as_ref()
                .map(|r| format!(": {}", format_type_hint(r)))
                .unwrap_or_default();
            let static_kw = if af.is_static { "static " } else { "" };
            Some(format!("{}fn({}){}", static_kw, params, ret))
        }
        ExprKind::Assign(a) => find_closure_in_expr(source, a.value, cursor_byte, word_len),
        ExprKind::FunctionCall(fc) => {
            if let Some(sig) = find_closure_in_expr(source, fc.name, cursor_byte, word_len) {
                return Some(sig);
            }
            for arg in fc.args.iter() {
                if let Some(sig) = find_closure_in_expr(source, &arg.value, cursor_byte, word_len) {
                    return Some(sig);
                }
            }
            None
        }
        ExprKind::MethodCall(mc) => {
            for arg in mc.args.iter() {
                if let Some(sig) = find_closure_in_expr(source, &arg.value, cursor_byte, word_len) {
                    return Some(sig);
                }
            }
            None
        }
        ExprKind::StaticMethodCall(smc) => {
            for arg in smc.args.iter() {
                if let Some(sig) = find_closure_in_expr(source, &arg.value, cursor_byte, word_len) {
                    return Some(sig);
                }
            }
            None
        }
        ExprKind::Parenthesized(inner) => {
            find_closure_in_expr(source, inner, cursor_byte, word_len)
        }
        _ => None,
    }
}

/// Return true when `span_start` is close enough to `cursor_byte` to be the
/// keyword the user is hovering.  The span starts at the first character of the
/// keyword (`function`/`fn`).
#[inline]
fn c_span_matches(span_start: u32, cursor_byte: u32, word_len: u32) -> bool {
    span_start <= cursor_byte && cursor_byte < span_start + word_len + 2
}
