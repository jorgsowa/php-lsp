/// Deep AST walker — collects all spans where `word` appears as a name reference
/// (function calls, `new Foo`, method calls, bare identifiers, static calls).
use php_ast::{
    ClassMemberKind, EnumMemberKind, Expr, ExprKind, NamespaceBody, Span, Stmt, StmtKind,
};

use crate::ast::str_offset;

pub fn refs_in_stmts(stmts: &[Stmt<'_, '_>], word: &str, out: &mut Vec<Span>) {
    for stmt in stmts {
        refs_in_stmt(stmt, word, out);
    }
}

/// Like `refs_in_stmts`, but also matches spans inside `use` statements.
/// Needed so that renaming a class also renames its `use` import.
pub fn refs_in_stmts_with_use(stmts: &[Stmt<'_, '_>], word: &str, out: &mut Vec<Span>) {
    refs_in_stmts(stmts, word, out);
    use_refs(stmts, word, out);
}

fn use_refs(stmts: &[Stmt<'_, '_>], word: &str, out: &mut Vec<Span>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) => {
                for use_item in u.uses.iter() {
                    let fqn = use_item.name.to_string_repr().into_owned();
                    let alias_match = use_item.alias.map(|a| a == word).unwrap_or(false);
                    let last_seg = fqn.rsplit('\\').next().unwrap_or(&fqn);
                    if alias_match || last_seg == word {
                        let name_span = use_item.name.span();
                        let offset = (fqn.len() - last_seg.len()) as u32;
                        let syn_span = Span {
                            start: name_span.start + offset,
                            end: name_span.start + fqn.len() as u32,
                        };
                        out.push(syn_span);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    use_refs(inner, word, out);
                }
            }
            _ => {}
        }
    }
}

pub fn refs_in_stmt(stmt: &Stmt<'_, '_>, word: &str, out: &mut Vec<Span>) {
    match &stmt.kind {
        StmtKind::Expression(e) => refs_in_expr(e, word, out),
        StmtKind::Return(Some(v)) => refs_in_expr(v, word, out),
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                refs_in_expr(expr, word, out);
            }
        }
        StmtKind::Function(f) => {
            if f.name == word {
                out.push(stmt.span);
            }
            refs_in_stmts(&f.body, word, out);
        }
        StmtKind::Class(c) => {
            if c.name == Some(word) {
                out.push(stmt.span);
            }
            for member in c.members.iter() {
                match &member.kind {
                    ClassMemberKind::Method(m) => {
                        if m.name == word {
                            out.push(member.span);
                        }
                        if let Some(body) = &m.body {
                            refs_in_stmts(body, word, out);
                        }
                    }
                    ClassMemberKind::Property(p) => {
                        if let Some(default) = &p.default {
                            refs_in_expr(default, word, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        StmtKind::Interface(i) => {
            if i.name == word {
                out.push(stmt.span);
            }
        }
        StmtKind::Trait(t) => {
            if t.name == word {
                out.push(stmt.span);
            }
            for member in t.members.iter() {
                match &member.kind {
                    ClassMemberKind::Method(m) => {
                        if m.name == word {
                            out.push(member.span);
                        }
                        if let Some(body) = &m.body {
                            refs_in_stmts(body, word, out);
                        }
                    }
                    ClassMemberKind::Property(p) => {
                        if let Some(default) = &p.default {
                            refs_in_expr(default, word, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        StmtKind::Enum(e) => {
            if e.name == word {
                out.push(stmt.span);
            }
            for member in e.members.iter() {
                match &member.kind {
                    EnumMemberKind::Method(m) => {
                        if m.name == word {
                            out.push(member.span);
                        }
                        if let Some(body) = &m.body {
                            refs_in_stmts(body, word, out);
                        }
                    }
                    EnumMemberKind::Case(c) => {
                        if let Some(value) = &c.value {
                            refs_in_expr(value, word, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                refs_in_stmts(inner, word, out);
            }
        }
        StmtKind::If(i) => {
            refs_in_expr(&i.condition, word, out);
            refs_in_stmt(i.then_branch, word, out);
            for ei in i.elseif_branches.iter() {
                refs_in_expr(&ei.condition, word, out);
                refs_in_stmt(&ei.body, word, out);
            }
            if let Some(e) = &i.else_branch {
                refs_in_stmt(e, word, out);
            }
        }
        StmtKind::While(w) => {
            refs_in_expr(&w.condition, word, out);
            refs_in_stmt(w.body, word, out);
        }
        StmtKind::DoWhile(d) => {
            refs_in_stmt(d.body, word, out);
            refs_in_expr(&d.condition, word, out);
        }
        StmtKind::Foreach(f) => {
            refs_in_expr(&f.expr, word, out);
            refs_in_stmt(f.body, word, out);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                refs_in_expr(e, word, out);
            }
            for cond in f.condition.iter() {
                refs_in_expr(cond, word, out);
            }
            for e in f.update.iter() {
                refs_in_expr(e, word, out);
            }
            refs_in_stmt(f.body, word, out);
        }
        StmtKind::TryCatch(t) => {
            refs_in_stmts(&t.body, word, out);
            for catch in t.catches.iter() {
                refs_in_stmts(&catch.body, word, out);
            }
            if let Some(finally) = &t.finally {
                refs_in_stmts(finally, word, out);
            }
        }
        StmtKind::Block(stmts) => refs_in_stmts(stmts, word, out),
        StmtKind::StaticVar(vars) => {
            for var in vars.iter() {
                if let Some(v) = &var.default {
                    refs_in_expr(v, word, out);
                }
            }
        }
        _ => {}
    }
}

// ── Variable rename helpers ───────────────────────────────────────────────────

/// Collect all spans where `$var_name` (the variable name WITHOUT `$`) appears as an
/// ExprKind::Variable within `stmts`. Stops at nested function/closure/arrow-function
/// scope boundaries so that `$x` in an inner function is not conflated with `$x` in
/// the outer function.
pub fn var_refs_in_stmts(stmts: &[Stmt<'_, '_>], var_name: &str, out: &mut Vec<Span>) {
    for stmt in stmts {
        var_refs_in_stmt(stmt, var_name, out);
    }
}

fn var_refs_in_stmt(stmt: &Stmt<'_, '_>, var_name: &str, out: &mut Vec<Span>) {
    match &stmt.kind {
        // Scope boundaries — do NOT recurse into these
        StmtKind::Function(_) | StmtKind::Class(_) | StmtKind::Trait(_) | StmtKind::Enum(_) => {}
        StmtKind::Expression(e) => var_refs_in_expr(e, var_name, out),
        StmtKind::Return(Some(e)) => var_refs_in_expr(e, var_name, out),
        StmtKind::Return(None) | StmtKind::Break(_) | StmtKind::Continue(_) => {}
        StmtKind::Echo(exprs) => {
            for e in exprs.iter() {
                var_refs_in_expr(e, var_name, out);
            }
        }
        StmtKind::If(i) => {
            var_refs_in_expr(&i.condition, var_name, out);
            var_refs_in_stmt(i.then_branch, var_name, out);
            for ei in i.elseif_branches.iter() {
                var_refs_in_expr(&ei.condition, var_name, out);
                var_refs_in_stmt(&ei.body, var_name, out);
            }
            if let Some(e) = &i.else_branch {
                var_refs_in_stmt(e, var_name, out);
            }
        }
        StmtKind::While(w) => {
            var_refs_in_expr(&w.condition, var_name, out);
            var_refs_in_stmt(w.body, var_name, out);
        }
        StmtKind::DoWhile(d) => {
            var_refs_in_stmt(d.body, var_name, out);
            var_refs_in_expr(&d.condition, var_name, out);
        }
        StmtKind::Foreach(f) => {
            var_refs_in_expr(&f.expr, var_name, out);
            if let Some(k) = &f.key {
                var_refs_in_expr(k, var_name, out);
            }
            var_refs_in_expr(&f.value, var_name, out);
            var_refs_in_stmt(f.body, var_name, out);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                var_refs_in_expr(e, var_name, out);
            }
            for cond in f.condition.iter() {
                var_refs_in_expr(cond, var_name, out);
            }
            for e in f.update.iter() {
                var_refs_in_expr(e, var_name, out);
            }
            var_refs_in_stmt(f.body, var_name, out);
        }
        StmtKind::TryCatch(t) => {
            var_refs_in_stmts(&t.body, var_name, out);
            for catch in t.catches.iter() {
                var_refs_in_stmts(&catch.body, var_name, out);
            }
            if let Some(finally) = &t.finally {
                var_refs_in_stmts(finally, var_name, out);
            }
        }
        StmtKind::Block(inner) => var_refs_in_stmts(inner, var_name, out),
        StmtKind::StaticVar(vars) => {
            for v in vars.iter() {
                if let Some(def) = &v.default {
                    var_refs_in_expr(def, var_name, out);
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                var_refs_in_stmts(inner, var_name, out);
            }
        }
        _ => {}
    }
}

fn var_refs_in_expr(expr: &Expr<'_, '_>, var_name: &str, out: &mut Vec<Span>) {
    match &expr.kind {
        ExprKind::Variable(name) => {
            if name.as_ref() == var_name {
                out.push(expr.span);
            }
        }
        ExprKind::Assign(a) => {
            var_refs_in_expr(a.target, var_name, out);
            var_refs_in_expr(a.value, var_name, out);
        }
        ExprKind::Binary(b) => {
            var_refs_in_expr(b.left, var_name, out);
            var_refs_in_expr(b.right, var_name, out);
        }
        ExprKind::UnaryPrefix(u) => var_refs_in_expr(u.operand, var_name, out),
        ExprKind::UnaryPostfix(u) => var_refs_in_expr(u.operand, var_name, out),
        ExprKind::Ternary(t) => {
            var_refs_in_expr(t.condition, var_name, out);
            if let Some(then_expr) = t.then_expr {
                var_refs_in_expr(then_expr, var_name, out);
            }
            var_refs_in_expr(t.else_expr, var_name, out);
        }
        ExprKind::NullCoalesce(n) => {
            var_refs_in_expr(n.left, var_name, out);
            var_refs_in_expr(n.right, var_name, out);
        }
        ExprKind::Parenthesized(e) => var_refs_in_expr(e, var_name, out),
        ExprKind::ErrorSuppress(e) => var_refs_in_expr(e, var_name, out),
        ExprKind::Cast(_, e) => var_refs_in_expr(e, var_name, out),
        ExprKind::Clone(e) => var_refs_in_expr(e, var_name, out),
        ExprKind::ThrowExpr(e) => var_refs_in_expr(e, var_name, out),
        ExprKind::Print(e) => var_refs_in_expr(e, var_name, out),
        ExprKind::Empty(e) => var_refs_in_expr(e, var_name, out),
        ExprKind::Eval(e) => var_refs_in_expr(e, var_name, out),
        ExprKind::FunctionCall(f) => {
            var_refs_in_expr(f.name, var_name, out);
            for a in f.args.iter() {
                var_refs_in_expr(&a.value, var_name, out);
            }
        }
        ExprKind::MethodCall(m) => {
            var_refs_in_expr(m.object, var_name, out);
            for a in m.args.iter() {
                var_refs_in_expr(&a.value, var_name, out);
            }
        }
        ExprKind::NullsafeMethodCall(m) => {
            var_refs_in_expr(m.object, var_name, out);
            for a in m.args.iter() {
                var_refs_in_expr(&a.value, var_name, out);
            }
        }
        ExprKind::StaticMethodCall(s) => {
            var_refs_in_expr(s.class, var_name, out);
            for a in s.args.iter() {
                var_refs_in_expr(&a.value, var_name, out);
            }
        }
        ExprKind::New(n) => {
            var_refs_in_expr(n.class, var_name, out);
            for a in n.args.iter() {
                var_refs_in_expr(&a.value, var_name, out);
            }
        }
        ExprKind::ArrayAccess(a) => {
            var_refs_in_expr(a.array, var_name, out);
            if let Some(idx) = a.index {
                var_refs_in_expr(idx, var_name, out);
            }
        }
        ExprKind::PropertyAccess(p) => var_refs_in_expr(p.object, var_name, out),
        ExprKind::NullsafePropertyAccess(p) => var_refs_in_expr(p.object, var_name, out),
        ExprKind::StaticPropertyAccess(s) => var_refs_in_expr(s.class, var_name, out),
        ExprKind::Yield(y) => {
            if let Some(k) = y.key {
                var_refs_in_expr(k, var_name, out);
            }
            if let Some(v) = y.value {
                var_refs_in_expr(v, var_name, out);
            }
        }
        ExprKind::Match(m) => {
            var_refs_in_expr(m.subject, var_name, out);
            for arm in m.arms.iter() {
                if let Some(conds) = &arm.conditions {
                    for c in conds.iter() {
                        var_refs_in_expr(c, var_name, out);
                    }
                }
                var_refs_in_expr(&arm.body, var_name, out);
            }
        }
        ExprKind::Array(elems) => {
            for el in elems.iter() {
                if let Some(k) = &el.key {
                    var_refs_in_expr(k, var_name, out);
                }
                var_refs_in_expr(&el.value, var_name, out);
            }
        }
        ExprKind::Isset(exprs) => {
            for e in exprs.iter() {
                var_refs_in_expr(e, var_name, out);
            }
        }
        ExprKind::Include(_, e) => var_refs_in_expr(e, var_name, out),
        ExprKind::Exit(Some(e)) => var_refs_in_expr(e, var_name, out),
        // Scope boundaries within expressions — do NOT recurse into these
        ExprKind::Closure(_) | ExprKind::ArrowFunction(_) => {}
        _ => {}
    }
}

/// Collect all `$var_name` spans within the innermost function/method scope
/// that contains `byte_off`. If `byte_off` is not inside any function, collects
/// from the top-level stmts (respecting scope boundaries). Also collects the
/// parameter declaration span when the variable is a parameter of the scope.
pub fn collect_var_refs_in_scope(
    stmts: &[Stmt<'_, '_>],
    var_name: &str,
    byte_off: usize,
    out: &mut Vec<Span>,
) {
    for stmt in stmts {
        if collect_in_fn_at(stmt, var_name, byte_off, out) {
            return;
        }
    }
    // Not inside any function — collect top-level
    var_refs_in_stmts(stmts, var_name, out);
}

/// Returns `true` if `stmt` is (or contains) the function/method that owns `byte_off`
/// and has populated `out` with variable + param spans for `var_name`.
fn collect_in_fn_at(
    stmt: &Stmt<'_, '_>,
    var_name: &str,
    byte_off: usize,
    out: &mut Vec<Span>,
) -> bool {
    match &stmt.kind {
        StmtKind::Function(f) => {
            if byte_off < stmt.span.start as usize || byte_off >= stmt.span.end as usize {
                return false;
            }
            // Check nested functions first.
            for inner in f.body.iter() {
                if collect_in_fn_at(inner, var_name, byte_off, out) {
                    return true;
                }
            }
            // This is the enclosing function — collect param + body refs.
            for p in f.params.iter() {
                if p.name == var_name {
                    out.push(p.span);
                }
            }
            var_refs_in_stmts(&f.body, var_name, out);
            true
        }
        StmtKind::Class(c) => {
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if byte_off < member.span.start as usize
                        || byte_off >= member.span.end as usize
                    {
                        continue;
                    }
                    if let Some(body) = &m.body {
                        for inner in body.iter() {
                            if collect_in_fn_at(inner, var_name, byte_off, out) {
                                return true;
                            }
                        }
                        for p in m.params.iter() {
                            if p.name == var_name {
                                out.push(p.span);
                            }
                        }
                        var_refs_in_stmts(body, var_name, out);
                    }
                    return true;
                }
            }
            false
        }
        StmtKind::Trait(t) => {
            for member in t.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if byte_off < member.span.start as usize
                        || byte_off >= member.span.end as usize
                    {
                        continue;
                    }
                    if let Some(body) = &m.body {
                        for inner in body.iter() {
                            if collect_in_fn_at(inner, var_name, byte_off, out) {
                                return true;
                            }
                        }
                        for p in m.params.iter() {
                            if p.name == var_name {
                                out.push(p.span);
                            }
                        }
                        var_refs_in_stmts(body, var_name, out);
                    }
                    return true;
                }
            }
            false
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                for s in inner.iter() {
                    if collect_in_fn_at(s, var_name, byte_off, out) {
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

// ── Property rename helpers ───────────────────────────────────────────────────

/// Collect all spans where `prop_name` is accessed (`->prop`, `?->prop`) or
/// declared as a class/trait property, across all statements.
/// Because `PropertyAccess.property` is a `&'src str` sub-slice of `source`,
/// we use `str_offset` (pointer arithmetic) to obtain its byte offset.
pub fn property_refs_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    prop_name: &str,
    out: &mut Vec<Span>,
) {
    for stmt in stmts {
        property_refs_in_stmt(source, stmt, prop_name, out);
    }
}

fn property_refs_in_stmt(
    source: &str,
    stmt: &Stmt<'_, '_>,
    prop_name: &str,
    out: &mut Vec<Span>,
) {
    match &stmt.kind {
        StmtKind::Expression(e) => property_refs_in_expr(source, e, prop_name, out),
        StmtKind::Return(Some(e)) => property_refs_in_expr(source, e, prop_name, out),
        StmtKind::Echo(exprs) => {
            for e in exprs.iter() {
                property_refs_in_expr(source, e, prop_name, out);
            }
        }
        StmtKind::Function(f) => {
            property_refs_in_stmts(source, &f.body, prop_name, out);
        }
        StmtKind::Class(c) => {
            for member in c.members.iter() {
                match &member.kind {
                    ClassMemberKind::Property(p) if p.name == prop_name => {
                        let offset = str_offset(source, p.name);
                        out.push(Span {
                            start: offset,
                            end: offset + p.name.len() as u32,
                        });
                    }
                    ClassMemberKind::Method(m) => {
                        if let Some(body) = &m.body {
                            property_refs_in_stmts(source, body, prop_name, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        StmtKind::Trait(t) => {
            for member in t.members.iter() {
                match &member.kind {
                    ClassMemberKind::Property(p) if p.name == prop_name => {
                        let offset = str_offset(source, p.name);
                        out.push(Span {
                            start: offset,
                            end: offset + p.name.len() as u32,
                        });
                    }
                    ClassMemberKind::Method(m) => {
                        if let Some(body) = &m.body {
                            property_refs_in_stmts(source, body, prop_name, out);
                        }
                    }
                    _ => {}
                }
            }
        }
        StmtKind::Enum(e) => {
            for member in e.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind {
                    if let Some(body) = &m.body {
                        property_refs_in_stmts(source, body, prop_name, out);
                    }
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                property_refs_in_stmts(source, inner, prop_name, out);
            }
        }
        StmtKind::If(i) => {
            property_refs_in_expr(source, &i.condition, prop_name, out);
            property_refs_in_stmt(source, i.then_branch, prop_name, out);
            for ei in i.elseif_branches.iter() {
                property_refs_in_expr(source, &ei.condition, prop_name, out);
                property_refs_in_stmt(source, &ei.body, prop_name, out);
            }
            if let Some(e) = &i.else_branch {
                property_refs_in_stmt(source, e, prop_name, out);
            }
        }
        StmtKind::While(w) => {
            property_refs_in_expr(source, &w.condition, prop_name, out);
            property_refs_in_stmt(source, w.body, prop_name, out);
        }
        StmtKind::DoWhile(d) => {
            property_refs_in_stmt(source, d.body, prop_name, out);
            property_refs_in_expr(source, &d.condition, prop_name, out);
        }
        StmtKind::Foreach(f) => {
            property_refs_in_expr(source, &f.expr, prop_name, out);
            property_refs_in_stmt(source, f.body, prop_name, out);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                property_refs_in_expr(source, e, prop_name, out);
            }
            for cond in f.condition.iter() {
                property_refs_in_expr(source, cond, prop_name, out);
            }
            for e in f.update.iter() {
                property_refs_in_expr(source, e, prop_name, out);
            }
            property_refs_in_stmt(source, f.body, prop_name, out);
        }
        StmtKind::TryCatch(t) => {
            property_refs_in_stmts(source, &t.body, prop_name, out);
            for catch in t.catches.iter() {
                property_refs_in_stmts(source, &catch.body, prop_name, out);
            }
            if let Some(finally) = &t.finally {
                property_refs_in_stmts(source, finally, prop_name, out);
            }
        }
        StmtKind::Block(inner) => property_refs_in_stmts(source, inner, prop_name, out),
        _ => {}
    }
}

fn property_refs_in_expr(
    source: &str,
    expr: &Expr<'_, '_>,
    prop_name: &str,
    out: &mut Vec<Span>,
) {
    match &expr.kind {
        ExprKind::PropertyAccess(p) => {
            property_refs_in_expr(source, p.object, prop_name, out);
            let span = p.property.span;
            let name_in_src = source
                .get(span.start as usize..span.end as usize)
                .unwrap_or("");
            if name_in_src == prop_name {
                out.push(span);
            }
        }
        ExprKind::NullsafePropertyAccess(p) => {
            property_refs_in_expr(source, p.object, prop_name, out);
            let span = p.property.span;
            let name_in_src = source
                .get(span.start as usize..span.end as usize)
                .unwrap_or("");
            if name_in_src == prop_name {
                out.push(span);
            }
        }
        ExprKind::Assign(a) => {
            property_refs_in_expr(source, a.target, prop_name, out);
            property_refs_in_expr(source, a.value, prop_name, out);
        }
        ExprKind::Binary(b) => {
            property_refs_in_expr(source, b.left, prop_name, out);
            property_refs_in_expr(source, b.right, prop_name, out);
        }
        ExprKind::UnaryPrefix(u) => property_refs_in_expr(source, u.operand, prop_name, out),
        ExprKind::UnaryPostfix(u) => property_refs_in_expr(source, u.operand, prop_name, out),
        ExprKind::Ternary(t) => {
            property_refs_in_expr(source, t.condition, prop_name, out);
            if let Some(then_expr) = t.then_expr {
                property_refs_in_expr(source, then_expr, prop_name, out);
            }
            property_refs_in_expr(source, t.else_expr, prop_name, out);
        }
        ExprKind::NullCoalesce(n) => {
            property_refs_in_expr(source, n.left, prop_name, out);
            property_refs_in_expr(source, n.right, prop_name, out);
        }
        ExprKind::Parenthesized(e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::ErrorSuppress(e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::Cast(_, e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::Clone(e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::ThrowExpr(e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::Print(e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::Empty(e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::Eval(e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::FunctionCall(f) => {
            property_refs_in_expr(source, f.name, prop_name, out);
            for a in f.args.iter() {
                property_refs_in_expr(source, &a.value, prop_name, out);
            }
        }
        ExprKind::MethodCall(m) => {
            property_refs_in_expr(source, m.object, prop_name, out);
            for a in m.args.iter() {
                property_refs_in_expr(source, &a.value, prop_name, out);
            }
        }
        ExprKind::NullsafeMethodCall(m) => {
            property_refs_in_expr(source, m.object, prop_name, out);
            for a in m.args.iter() {
                property_refs_in_expr(source, &a.value, prop_name, out);
            }
        }
        ExprKind::StaticMethodCall(s) => {
            property_refs_in_expr(source, s.class, prop_name, out);
            for a in s.args.iter() {
                property_refs_in_expr(source, &a.value, prop_name, out);
            }
        }
        ExprKind::New(n) => {
            property_refs_in_expr(source, n.class, prop_name, out);
            for a in n.args.iter() {
                property_refs_in_expr(source, &a.value, prop_name, out);
            }
        }
        ExprKind::ArrayAccess(a) => {
            property_refs_in_expr(source, a.array, prop_name, out);
            if let Some(idx) = a.index {
                property_refs_in_expr(source, idx, prop_name, out);
            }
        }
        ExprKind::Yield(y) => {
            if let Some(k) = y.key {
                property_refs_in_expr(source, k, prop_name, out);
            }
            if let Some(v) = y.value {
                property_refs_in_expr(source, v, prop_name, out);
            }
        }
        ExprKind::Match(m) => {
            property_refs_in_expr(source, m.subject, prop_name, out);
            for arm in m.arms.iter() {
                if let Some(conds) = &arm.conditions {
                    for c in conds.iter() {
                        property_refs_in_expr(source, c, prop_name, out);
                    }
                }
                property_refs_in_expr(source, &arm.body, prop_name, out);
            }
        }
        ExprKind::Array(elems) => {
            for el in elems.iter() {
                if let Some(k) = &el.key {
                    property_refs_in_expr(source, k, prop_name, out);
                }
                property_refs_in_expr(source, &el.value, prop_name, out);
            }
        }
        ExprKind::Isset(exprs) => {
            for e in exprs.iter() {
                property_refs_in_expr(source, e, prop_name, out);
            }
        }
        ExprKind::Include(_, e) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::Exit(Some(e)) => property_refs_in_expr(source, e, prop_name, out),
        ExprKind::Closure(c) => property_refs_in_stmts(source, &c.body, prop_name, out),
        ExprKind::ArrowFunction(a) => property_refs_in_expr(source, a.body, prop_name, out),
        _ => {}
    }
}

fn args(arg_list: &[php_ast::Arg<'_, '_>], word: &str, out: &mut Vec<Span>) {
    for a in arg_list.iter() {
        refs_in_expr(&a.value, word, out);
    }
}

pub fn refs_in_expr(expr: &Expr<'_, '_>, word: &str, out: &mut Vec<Span>) {
    match &expr.kind {
        ExprKind::Identifier(name) => {
            if name.as_ref() == word {
                out.push(expr.span);
            }
        }
        ExprKind::FunctionCall(f) => {
            refs_in_expr(f.name, word, out);
            args(&f.args, word, out);
        }
        ExprKind::MethodCall(m) => {
            refs_in_expr(m.object, word, out);
            refs_in_expr(m.method, word, out);
            args(&m.args, word, out);
        }
        ExprKind::NullsafeMethodCall(m) => {
            refs_in_expr(m.object, word, out);
            refs_in_expr(m.method, word, out);
            args(&m.args, word, out);
        }
        ExprKind::StaticMethodCall(s) => {
            refs_in_expr(s.class, word, out);
            if s.method.as_ref() == word {
                out.push(expr.span);
            }
            args(&s.args, word, out);
        }
        ExprKind::New(n) => {
            refs_in_expr(n.class, word, out);
            args(&n.args, word, out);
        }
        ExprKind::Assign(a) => {
            refs_in_expr(a.target, word, out);
            refs_in_expr(a.value, word, out);
        }
        ExprKind::Binary(b) => {
            refs_in_expr(b.left, word, out);
            refs_in_expr(b.right, word, out);
        }
        ExprKind::UnaryPrefix(u) => refs_in_expr(u.operand, word, out),
        ExprKind::UnaryPostfix(u) => refs_in_expr(u.operand, word, out),
        ExprKind::Ternary(t) => {
            refs_in_expr(t.condition, word, out);
            if let Some(then_expr) = t.then_expr {
                refs_in_expr(then_expr, word, out);
            }
            refs_in_expr(t.else_expr, word, out);
        }
        ExprKind::NullCoalesce(n) => {
            refs_in_expr(n.left, word, out);
            refs_in_expr(n.right, word, out);
        }
        ExprKind::Parenthesized(e) => refs_in_expr(e, word, out),
        ExprKind::ErrorSuppress(e) => refs_in_expr(e, word, out),
        ExprKind::Cast(_, e) => refs_in_expr(e, word, out),
        ExprKind::Clone(e) => refs_in_expr(e, word, out),
        ExprKind::ThrowExpr(e) => refs_in_expr(e, word, out),
        ExprKind::Print(e) => refs_in_expr(e, word, out),
        ExprKind::Empty(e) => refs_in_expr(e, word, out),
        ExprKind::Eval(e) => refs_in_expr(e, word, out),
        ExprKind::Yield(y) => {
            if let Some(k) = y.key {
                refs_in_expr(k, word, out);
            }
            if let Some(v) = y.value {
                refs_in_expr(v, word, out);
            }
        }
        ExprKind::ArrayAccess(a) => {
            refs_in_expr(a.array, word, out);
            if let Some(idx) = a.index {
                refs_in_expr(idx, word, out);
            }
        }
        ExprKind::PropertyAccess(p) => refs_in_expr(p.object, word, out),
        ExprKind::NullsafePropertyAccess(p) => refs_in_expr(p.object, word, out),
        ExprKind::StaticPropertyAccess(s) => refs_in_expr(s.class, word, out),
        ExprKind::ClassConstAccess(c) => {
            refs_in_expr(c.class, word, out);
            if c.member.as_ref() == word {
                out.push(expr.span);
            }
        }
        ExprKind::Closure(c) => refs_in_stmts(&c.body, word, out),
        ExprKind::ArrowFunction(a) => refs_in_expr(a.body, word, out),
        ExprKind::Match(m) => {
            refs_in_expr(m.subject, word, out);
            for arm in m.arms.iter() {
                if let Some(conds) = &arm.conditions {
                    for cond in conds.iter() {
                        refs_in_expr(cond, word, out);
                    }
                }
                refs_in_expr(&arm.body, word, out);
            }
        }
        ExprKind::Array(elements) => {
            for elem in elements.iter() {
                if let Some(key) = &elem.key {
                    refs_in_expr(key, word, out);
                }
                refs_in_expr(&elem.value, word, out);
            }
        }
        ExprKind::Isset(exprs) => {
            for e in exprs.iter() {
                refs_in_expr(e, word, out);
            }
        }
        ExprKind::Include(_, e) => refs_in_expr(e, word, out),
        ExprKind::Exit(Some(e)) => refs_in_expr(e, word, out),
        ExprKind::AnonymousClass(c) => {
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    refs_in_stmts(body, word, out);
                }
            }
        }
        _ => {}
    }
}
