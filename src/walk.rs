/// Deep AST walker — collects all spans where `word` appears as a name reference
/// (function calls, `new Foo`, method calls, bare identifiers, static calls).
use php_ast::{ClassMemberKind, EnumMemberKind, Expr, ExprKind, NamespaceBody, Span, Stmt, StmtKind};

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
        StmtKind::Return(r) => {
            if let Some(v) = r {
                refs_in_expr(v, word, out);
            }
        }
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
                if let ClassMemberKind::Method(m) = &member.kind {
                    if let Some(body) = &m.body {
                        refs_in_stmts(body, word, out);
                    }
                }
            }
        }
        _ => {}
    }
}
