use php_ast::{ClassMemberKind, Expr, ExprKind, NamespaceBody, Stmt, StmtKind};

/// Hooks for [`walk_stmts_at`]; a hook returning `Some` replaces the default
/// descent into that node, and `true` anywhere stops the walk.
pub(super) trait CursorVisitor {
    fn visit_stmt(&mut self, _stmt: &Stmt<'_, '_>, _cursor: u32) -> Option<bool> {
        None
    }

    fn visit_expr(&mut self, _expr: &Expr<'_, '_>, _cursor: u32) -> Option<bool> {
        None
    }
}

fn contains(span: php_ast::Span, cursor: u32) -> bool {
    span.start <= cursor && cursor <= span.end
}

/// Descends into the statements of `stmts` whose span contains `cursor`.
pub(super) fn walk_stmts_at(
    stmts: &[Stmt<'_, '_>],
    cursor: u32,
    v: &mut impl CursorVisitor,
) -> bool {
    stmts
        .iter()
        .filter(|s| contains(s.span, cursor))
        .any(|s| walk_stmt_at(s, cursor, v))
}

fn walk_stmt_at(stmt: &Stmt<'_, '_>, cursor: u32, v: &mut impl CursorVisitor) -> bool {
    if let Some(done) = v.visit_stmt(stmt, cursor) {
        return done;
    }
    match &stmt.kind {
        StmtKind::Expression(expr) | StmtKind::Throw(expr) | StmtKind::Return(Some(expr)) => {
            walk_expr_at(expr, cursor, v)
        }
        StmtKind::Function(f) => walk_stmts_at(&f.body.stmts, cursor, v),
        StmtKind::Class(c) => c.body.members.iter().any(|member| {
            matches!(&member.kind, ClassMemberKind::Method(m)
                if m.body.as_ref().is_some_and(|body| walk_stmts_at(&body.stmts, cursor, v)))
        }),
        StmtKind::Namespace(ns) => match &ns.body {
            NamespaceBody::Braced(inner) => walk_stmts_at(&inner.stmts, cursor, v),
            _ => false,
        },
        StmtKind::Block(b) => walk_stmts_at(&b.stmts, cursor, v),
        StmtKind::If(i) => {
            walk_expr_at(&i.condition, cursor, v)
                || walk_stmt_at(i.then_branch, cursor, v)
                || i.elseif_branches.iter().any(|ei| {
                    walk_expr_at(&ei.condition, cursor, v) || walk_stmt_at(&ei.body, cursor, v)
                })
                || i.else_branch
                    .as_ref()
                    .is_some_and(|e| walk_stmt_at(e, cursor, v))
        }
        StmtKind::Switch(sw) => {
            walk_expr_at(&sw.expr, cursor, v)
                || sw
                    .body
                    .cases
                    .iter()
                    .any(|case| walk_stmts_at(&case.body, cursor, v))
        }
        StmtKind::While(w) => {
            walk_expr_at(&w.condition, cursor, v) || walk_stmt_at(w.body, cursor, v)
        }
        StmtKind::DoWhile(d) => {
            walk_stmt_at(d.body, cursor, v) || walk_expr_at(&d.condition, cursor, v)
        }
        StmtKind::For(f) => {
            f.init
                .iter()
                .chain(f.condition.iter())
                .chain(f.update.iter())
                .any(|e| walk_expr_at(e, cursor, v))
                || walk_stmt_at(f.body, cursor, v)
        }
        StmtKind::Foreach(f) => walk_expr_at(&f.expr, cursor, v) || walk_stmt_at(f.body, cursor, v),
        StmtKind::TryCatch(t) => {
            walk_stmts_at(&t.body.stmts, cursor, v)
                || t.catches
                    .iter()
                    .any(|catch| walk_stmts_at(&catch.body.stmts, cursor, v))
                || t.finally
                    .as_ref()
                    .is_some_and(|finally| walk_stmts_at(&finally.stmts, cursor, v))
        }
        _ => false,
    }
}

/// Descends into `expr` if its span contains `cursor`.
pub(super) fn walk_expr_at(expr: &Expr<'_, '_>, cursor: u32, v: &mut impl CursorVisitor) -> bool {
    if !contains(expr.span, cursor) {
        return false;
    }
    if let Some(done) = v.visit_expr(expr, cursor) {
        return done;
    }
    match &expr.kind {
        ExprKind::Closure(c) => walk_stmts_at(&c.body.stmts, cursor, v),
        ExprKind::ArrowFunction(af) => walk_expr_at(af.body, cursor, v),
        ExprKind::Assign(a) => walk_expr_at(a.value, cursor, v),
        ExprKind::FunctionCall(fc) => {
            walk_expr_at(fc.name, cursor, v)
                || fc
                    .args
                    .iter()
                    .filter_map(|arg| arg.value())
                    .any(|value| walk_expr_at(value, cursor, v))
        }
        ExprKind::MethodCall(mc) | ExprKind::NullsafeMethodCall(mc) => mc
            .args
            .iter()
            .filter_map(|arg| arg.value())
            .any(|value| walk_expr_at(value, cursor, v)),
        ExprKind::StaticMethodCall(smc) => smc
            .args
            .iter()
            .filter_map(|arg| arg.value())
            .any(|value| walk_expr_at(value, cursor, v)),
        ExprKind::Parenthesized(inner) => walk_expr_at(inner, cursor, v),
        _ => false,
    }
}
