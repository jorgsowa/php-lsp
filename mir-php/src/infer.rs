/// Type inference: builds a `TypeEnv` from an AST.
///
/// Handles:
/// - `$var = new ClassName()` → `Object("ClassName")`
/// - `$var = "..."` → `Str`
/// - `$var = 42` → `Int`
/// - `$var = 3.14` → `Float`
/// - `$var = true/false` → `Bool`
/// - `$var = null` → `Null`
/// - Typed parameters: `function foo(Foo $bar)` → `$bar: Object("Foo")`
/// - Typed parameters: `function foo(int $n)` → `$n: Int`
use php_ast::{ClassMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind, TypeHintKind};

use crate::types::{Ty, TypeEnv};

/// Build a `TypeEnv` from the top-level statements of a parsed document.
pub fn infer<'a>(stmts: &[Stmt<'a, 'a>]) -> TypeEnv {
    let mut env = TypeEnv::default();
    collect_stmts(stmts, &mut env);
    env
}

fn collect_stmts<'a>(stmts: &[Stmt<'a, 'a>], env: &mut TypeEnv) {
    for stmt in stmts {
        collect_stmt(stmt, env);
    }
}

fn collect_stmt<'a>(stmt: &Stmt<'a, 'a>, env: &mut TypeEnv) {
    match &stmt.kind {
        StmtKind::Expression(e) => collect_expr(e, env),
        StmtKind::Function(f) => {
            for p in f.params.iter() {
                if let Some(ty) = param_type(p) {
                    env.insert(format!("${}", p.name), ty);
                }
            }
            collect_stmts(&f.body, env);
        }
        StmtKind::Class(c) => {
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    for p in m.params.iter() {
                        if let Some(ty) = param_type(p) {
                            env.insert(format!("${}", p.name), ty);
                        }
                    }
                    if let Some(body) = &m.body {
                        collect_stmts(body, env);
                    }
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                collect_stmts(inner, env);
            }
        }
        _ => {}
    }
}

fn collect_expr<'a>(expr: &php_ast::Expr<'a, 'a>, env: &mut TypeEnv) {
    if let ExprKind::Assign(assign) = &expr.kind {
        if let ExprKind::Variable(var_name) = &assign.target.kind {
            let key = format!("${}", var_name);
            let ty = infer_expr(&assign.value);
            if !matches!(ty, Ty::Unknown) {
                env.insert(key, ty);
            }
        }
        collect_expr(assign.value, env);
    }
}

fn infer_expr<'a>(expr: &php_ast::Expr<'a, 'a>) -> Ty {
    match &expr.kind {
        ExprKind::New(n) => {
            if let ExprKind::Identifier(name) = &n.class.kind {
                return Ty::Object(name.to_string());
            }
            Ty::Unknown
        }
        ExprKind::String(_) => Ty::Str,
        ExprKind::Int(_) => Ty::Int,
        ExprKind::Float(_) => Ty::Float,
        ExprKind::Bool(_) => Ty::Bool,
        ExprKind::Null => Ty::Null,
        ExprKind::Parenthesized(e) => infer_expr(e),
        _ => Ty::Unknown,
    }
}

fn param_type<'a>(p: &php_ast::Param<'a, 'a>) -> Option<Ty> {
    let hint = p.type_hint.as_ref()?;
    Some(hint_to_ty(&hint.kind))
}

fn hint_to_ty<'a>(kind: &TypeHintKind<'a, 'a>) -> Ty {
    match kind {
        TypeHintKind::Named(name) => {
            let s = name.to_string_repr();
            Ty::from_str(s.as_ref())
        }
        TypeHintKind::Keyword(builtin, _) => Ty::from_str(builtin.as_str()),
        TypeHintKind::Nullable(inner) => Ty::Union(vec![hint_to_ty(&inner.kind), Ty::Null]),
        TypeHintKind::Union(types) => {
            Ty::Union(types.iter().map(|t| hint_to_ty(&t.kind)).collect())
        }
        TypeHintKind::Intersection(types) => {
            Ty::Intersection(types.iter().map(|t| hint_to_ty(&t.kind)).collect())
        }
    }
}
