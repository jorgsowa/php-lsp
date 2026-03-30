/// Semantic diagnostics: undefined symbols, arity mismatches, undefined
/// variables, return-type mismatches, and null-safety violations.
///
/// This module is the core of mir-php's static analysis. It operates on raw
/// AST slices and produces `Diagnostic` values with no dependency on tower-lsp.
use std::collections::{HashMap, HashSet};

use php_ast::{ClassMemberKind, Expr, ExprKind, NamespaceBody, Span, Stmt, StmtKind};

use crate::stubs;
use crate::util::{format_type_hint, offset_to_position};

// ── Public diagnostic types ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Diagnostic {
    pub start_line: u32,
    pub start_char: u32,
    pub end_line: u32,
    pub end_char: u32,
    pub severity: Severity,
    pub message: String,
    /// Same-file related locations: `(start_line, start_char, end_line, end_char, message)`.
    /// Used to link arity warnings back to the function declaration.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<(u32, u32, u32, u32, String)>,
}

// ── Internal arity bookkeeping ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct Arity {
    required: usize,
    max: usize, // usize::MAX for variadic
}

/// Declaration location for a user-defined function (0-based line + char of the `function` keyword).
#[derive(Debug, Clone, Copy, Default)]
struct DeclLoc {
    line: u32,
    char: u32,
}

#[derive(Debug, Clone, Copy)]
struct DefEntry {
    arity: Arity,
    /// `Some` only for user-defined functions declared in the current document.
    decl: Option<DeclLoc>,
}

type DefMap = HashMap<String, DefEntry>;

// ── Public entry point ────────────────────────────────────────────────────────

/// Analyse `stmts` (the document being checked) against `all` (every document
/// in the workspace, including the current one) and return diagnostics.
pub fn analyze<'a>(
    source: &str,
    stmts: &[Stmt<'a, 'a>],
    all: &[(&str, &[Stmt<'a, 'a>])],
) -> Vec<Diagnostic> {
    let mut defs: DefMap = HashMap::new();

    // Built-in functions are pre-loaded so they are never flagged as undefined.
    for &(name, req, max) in stubs::BUILTIN_FUNCTIONS {
        defs.insert(name.to_string(), DefEntry { arity: Arity { required: req, max }, decl: None });
    }

    // Collect user-defined functions and classes from all documents.
    // The first entry in `all` is the current document — collect its decl locations.
    for (i, (doc_src, doc_stmts)) in all.iter().enumerate() {
        let is_current = i == 0;
        collect_defs(doc_stmts, if is_current { Some(doc_src) } else { None }, &mut defs);
    }

    let mut out = Vec::new();
    check_stmts(source, stmts, &defs, &mut out);
    out
}

// ── Definition collection ─────────────────────────────────────────────────────

/// Collect function/class/method definitions into `defs`.
/// If `source` is `Some`, declaration locations are computed and stored.
fn collect_defs<'a>(stmts: &[Stmt<'a, 'a>], source: Option<&&str>, defs: &mut DefMap) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let decl = source.map(|src| {
                    let (line, char) = offset_to_position(src, stmt.span.start);
                    DeclLoc { line, char }
                });
                defs.insert(f.name.to_string(), DefEntry { arity: params_arity(&f.params), decl });
            }
            StmtKind::Class(c) => {
                let class_name = c.name.unwrap_or("");
                defs.insert(
                    class_name.to_string(),
                    DefEntry { arity: Arity { required: 0, max: 0 }, decl: None },
                );
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let arity = params_arity(&m.params);
                        let decl = source.map(|src| {
                            let (line, char) = offset_to_position(src, member.span.start);
                            DeclLoc { line, char }
                        });
                        if !class_name.is_empty() {
                            defs.insert(format!("{}::{}", class_name, m.name), DefEntry { arity, decl });
                        }
                        defs.insert(m.name.to_string(), DefEntry { arity, decl });
                    }
                }
            }
            StmtKind::Interface(i) => {
                defs.insert(
                    i.name.to_string(),
                    DefEntry { arity: Arity { required: 0, max: 0 }, decl: None },
                );
            }
            StmtKind::Trait(t) => {
                defs.insert(
                    t.name.to_string(),
                    DefEntry { arity: Arity { required: 0, max: 0 }, decl: None },
                );
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_defs(inner, source, defs);
                }
            }
            _ => {}
        }
    }
}

fn params_arity<'a>(params: &[php_ast::Param<'a, 'a>]) -> Arity {
    let mut required = 0usize;
    let mut is_variadic = false;
    for p in params.iter() {
        if p.variadic {
            is_variadic = true;
        } else if p.default.is_none() {
            required += 1;
        }
    }
    Arity {
        required,
        max: if is_variadic { usize::MAX } else { params.len() },
    }
}

// ── Statement checking ────────────────────────────────────────────────────────

fn check_stmts<'a>(
    source: &str,
    stmts: &[Stmt<'a, 'a>],
    defs: &DefMap,
    out: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        check_stmt(source, stmt, defs, out);
    }
}

fn check_stmt<'a>(
    source: &str,
    stmt: &Stmt<'a, 'a>,
    defs: &DefMap,
    out: &mut Vec<Diagnostic>,
) {
    match &stmt.kind {
        StmtKind::Expression(e) => check_expr(source, e, defs, out),
        StmtKind::Return(r) => {
            if let Some(v) = r {
                check_expr(source, v, defs, out);
            }
        }
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                check_expr(source, expr, defs, out);
            }
        }
        StmtKind::Function(f) => {
            check_stmts(source, &f.body, defs, out);
            check_function_vars(source, f.params.as_ref(), &f.body, out);
            if let Some(ret) = &f.return_type {
                let ret_str = format_type_hint(ret);
                check_return_types_in_body(source, &f.body, &ret_str, out);
            }
        }
        StmtKind::Class(c) => {
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if let Some(body) = &m.body {
                        check_stmts(source, body, defs, out);
                        check_function_vars(source, m.params.as_ref(), body, out);
                        if let Some(ret) = &m.return_type {
                            let ret_str = format_type_hint(ret);
                            check_return_types_in_body(source, body, &ret_str, out);
                        }
                    }
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                check_stmts(source, inner, defs, out);
            }
        }
        _ => {}
    }
}

// ── Undefined variable detection ──────────────────────────────────────────────

const SUPERGLOBALS: &[&str] = &[
    "GLOBALS", "_GET", "_POST", "_SESSION", "_SERVER", "_ENV", "_COOKIE", "_FILES", "_REQUEST",
];

fn check_function_vars<'a>(
    source: &str,
    params: &[php_ast::Param<'a, 'a>],
    body: &[Stmt<'a, 'a>],
    out: &mut Vec<Diagnostic>,
) {
    if uses_global_or_extract(body) {
        return;
    }

    let mut defined: HashSet<String> = HashSet::new();
    defined.insert("this".to_string());
    for sg in SUPERGLOBALS {
        defined.insert(sg.to_string());
    }
    for p in params {
        defined.insert(p.name.to_string());
    }
    collect_defined_in_stmts(body, &mut defined);
    check_var_uses_in_stmts(source, body, &defined, out);
}

fn uses_global_or_extract<'a>(stmts: &[Stmt<'a, 'a>]) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Global(_) | StmtKind::StaticVar(_) => return true,
            StmtKind::Expression(e) => {
                if let ExprKind::FunctionCall(f) = &e.kind {
                    if let Some(name) = simple_fn_name(f.name) {
                        if name == "extract" || name == "compact" {
                            return true;
                        }
                    }
                }
            }
            StmtKind::If(i) => {
                if uses_global_or_extract(std::slice::from_ref(i.then_branch))
                    || i.elseif_branches
                        .iter()
                        .any(|b| uses_global_or_extract(std::slice::from_ref(&b.body)))
                    || i.else_branch
                        .map_or(false, |b| uses_global_or_extract(std::slice::from_ref(b)))
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn simple_fn_name<'a>(expr: &'a Expr<'_, '_>) -> Option<&'a str> {
    if let ExprKind::Identifier(n) = &expr.kind {
        Some(n.as_ref())
    } else {
        None
    }
}

fn collect_defined_in_stmts<'a>(stmts: &[Stmt<'a, 'a>], defined: &mut HashSet<String>) {
    for stmt in stmts {
        collect_defined_in_stmt(stmt, defined);
    }
}

fn collect_defined_in_stmt<'a>(stmt: &Stmt<'a, 'a>, defined: &mut HashSet<String>) {
    match &stmt.kind {
        StmtKind::Expression(e) => collect_defined_in_expr(e, defined),
        StmtKind::Return(Some(e)) => collect_defined_in_expr(e, defined),
        StmtKind::Echo(exprs) => {
            for e in exprs.iter() {
                collect_defined_in_expr(e, defined);
            }
        }
        StmtKind::If(i) => {
            collect_defined_in_expr(&i.condition, defined);
            collect_defined_in_stmt(i.then_branch, defined);
            for b in i.elseif_branches.iter() {
                collect_defined_in_stmt(&b.body, defined);
            }
            if let Some(b) = i.else_branch {
                collect_defined_in_stmt(b, defined);
            }
        }
        StmtKind::While(w) => {
            collect_defined_in_expr(&w.condition, defined);
            collect_defined_in_stmt(&w.body, defined);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                collect_defined_in_expr(e, defined);
            }
            for e in f.condition.iter() {
                collect_defined_in_expr(e, defined);
            }
            for e in f.update.iter() {
                collect_defined_in_expr(e, defined);
            }
            collect_defined_in_stmt(&f.body, defined);
        }
        StmtKind::Foreach(f) => {
            if let ExprKind::Variable(k) =
                &f.key.as_ref().map(|k| &k.kind).unwrap_or(&ExprKind::Null)
            {
                defined.insert(k.to_string());
            }
            if let ExprKind::Variable(v) = &f.value.kind {
                defined.insert(v.to_string());
            }
            collect_defined_in_stmt(f.body, defined);
        }
        StmtKind::TryCatch(t) => {
            collect_defined_in_stmts(&t.body, defined);
            for catch in t.catches.iter() {
                if let Some(v) = catch.var {
                    defined.insert(v.to_string());
                }
                collect_defined_in_stmts(&catch.body, defined);
            }
        }
        StmtKind::Block(stmts) => collect_defined_in_stmts(stmts, defined),
        _ => {}
    }
}

fn collect_defined_in_expr<'a>(expr: &Expr<'a, 'a>, defined: &mut HashSet<String>) {
    match &expr.kind {
        ExprKind::Assign(a) => {
            if let ExprKind::Variable(v) = &a.target.kind {
                defined.insert(v.to_string());
            }
            collect_defined_in_expr(a.value, defined);
        }
        ExprKind::Parenthesized(e) => collect_defined_in_expr(e, defined),
        _ => {}
    }
}

fn check_var_uses_in_stmts<'a>(
    source: &str,
    stmts: &[Stmt<'a, 'a>],
    defined: &HashSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        check_var_uses_in_stmt(source, stmt, defined, out);
    }
}

fn check_var_uses_in_stmt<'a>(
    source: &str,
    stmt: &Stmt<'a, 'a>,
    defined: &HashSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    match &stmt.kind {
        StmtKind::Expression(e) => check_var_uses_in_expr(source, e, defined, out),
        StmtKind::Return(Some(e)) => check_var_uses_in_expr(source, e, defined, out),
        StmtKind::Echo(exprs) => {
            for e in exprs.iter() {
                check_var_uses_in_expr(source, e, defined, out);
            }
        }
        StmtKind::If(i) => {
            check_var_uses_in_expr(source, &i.condition, defined, out);
            check_var_uses_in_stmt(source, i.then_branch, defined, out);
            for b in i.elseif_branches.iter() {
                check_var_uses_in_stmt(source, &b.body, defined, out);
            }
            if let Some(b) = i.else_branch {
                check_var_uses_in_stmt(source, b, defined, out);
            }
        }
        StmtKind::While(w) => {
            check_var_uses_in_expr(source, &w.condition, defined, out);
            check_var_uses_in_stmt(source, &w.body, defined, out);
        }
        StmtKind::For(f) => {
            for e in f.condition.iter() {
                check_var_uses_in_expr(source, e, defined, out);
            }
            for e in f.update.iter() {
                check_var_uses_in_expr(source, e, defined, out);
            }
            check_var_uses_in_stmt(source, &f.body, defined, out);
        }
        StmtKind::Foreach(f) => {
            check_var_uses_in_expr(source, &f.expr, defined, out);
            check_var_uses_in_stmt(source, f.body, defined, out);
        }
        StmtKind::TryCatch(t) => {
            check_var_uses_in_stmts(source, &t.body, defined, out);
            for catch in t.catches.iter() {
                check_var_uses_in_stmts(source, &catch.body, defined, out);
            }
        }
        StmtKind::Block(stmts) => check_var_uses_in_stmts(source, stmts, defined, out),
        _ => {}
    }
}

fn check_var_uses_in_expr<'a>(
    source: &str,
    expr: &Expr<'a, 'a>,
    defined: &HashSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    match &expr.kind {
        ExprKind::Variable(name) => {
            let n = name.as_ref();
            if !defined.contains(n) {
                let (sl, sc) = offset_to_position(source, expr.span.start);
                out.push(Diagnostic {
                    start_line: sl,
                    start_char: sc,
                    end_line: sl,
                    end_char: sc + n.len() as u32 + 1, // +1 for '$'
                    severity: Severity::Hint,
                    message: format!("Undefined variable: ${n}"),
                    related: vec![],
                });
            }
        }
        ExprKind::Assign(a) => {
            check_var_uses_in_expr(source, a.value, defined, out);
        }
        ExprKind::FunctionCall(f) => {
            for arg in f.args.iter() {
                check_var_uses_in_expr(source, &arg.value, defined, out);
            }
        }
        ExprKind::MethodCall(m) => {
            check_var_uses_in_expr(source, m.object, defined, out);
            for arg in m.args.iter() {
                check_var_uses_in_expr(source, &arg.value, defined, out);
            }
        }
        ExprKind::New(n) => {
            for arg in n.args.iter() {
                check_var_uses_in_expr(source, &arg.value, defined, out);
            }
        }
        ExprKind::Parenthesized(e) => check_var_uses_in_expr(source, e, defined, out),
        ExprKind::Ternary(t) => {
            check_var_uses_in_expr(source, t.condition, defined, out);
            if let Some(e) = t.then_expr {
                check_var_uses_in_expr(source, e, defined, out);
            }
            check_var_uses_in_expr(source, t.else_expr, defined, out);
        }
        ExprKind::Binary(b) => {
            check_var_uses_in_expr(source, b.left, defined, out);
            check_var_uses_in_expr(source, b.right, defined, out);
        }
        ExprKind::UnaryPrefix(u) => check_var_uses_in_expr(source, u.operand, defined, out),
        ExprKind::UnaryPostfix(u) => check_var_uses_in_expr(source, u.operand, defined, out),
        ExprKind::PropertyAccess(p) => check_var_uses_in_expr(source, p.object, defined, out),
        ExprKind::ArrayAccess(a) => {
            check_var_uses_in_expr(source, a.array, defined, out);
            if let Some(idx) = a.index {
                check_var_uses_in_expr(source, idx, defined, out);
            }
        }
        _ => {}
    }
}

// ── Call checking ─────────────────────────────────────────────────────────────

fn check_expr<'a>(source: &str, expr: &Expr<'a, 'a>, defs: &DefMap, out: &mut Vec<Diagnostic>) {
    match &expr.kind {
        ExprKind::FunctionCall(f) => {
            if let Some(name) = simple_ident_name(f.name) {
                let arg_count = f.args.len();
                if let Some(entry) = defs.get(&name) {
                    if arg_count < entry.arity.required || arg_count > entry.arity.max {
                        out.push(arity_diag(source, f.name.span, &name, entry.arity.required, entry.arity.max, arg_count, entry.decl));
                    }
                } else {
                    out.push(undefined_diag(source, f.name.span, &name));
                }
            }
            for arg in f.args.iter() {
                check_expr(source, &arg.value, defs, out);
            }
        }
        ExprKind::MethodCall(m) => {
            if matches!(m.object.kind, ExprKind::Null) {
                let (sl, sc) = offset_to_position(source, m.object.span.start);
                out.push(Diagnostic {
                    start_line: sl,
                    start_char: sc,
                    end_line: sl,
                    end_char: sc + 4,
                    severity: Severity::Warning,
                    message: "Calling a method on null".to_string(),
                    related: vec![],
                });
            }
            check_expr(source, m.object, defs, out);
            for arg in m.args.iter() {
                check_expr(source, &arg.value, defs, out);
            }
        }
        ExprKind::New(n) => {
            if let Some(name) = simple_ident_name(n.class) {
                if !defs.contains_key(&name) && !stubs::is_builtin_class(&name) {
                    out.push(undefined_diag(source, n.class.span, &name));
                }
            }
            for arg in n.args.iter() {
                check_expr(source, &arg.value, defs, out);
            }
        }
        ExprKind::Assign(a) => {
            check_expr(source, a.target, defs, out);
            check_expr(source, a.value, defs, out);
        }
        ExprKind::Parenthesized(e) => check_expr(source, e, defs, out),
        ExprKind::Ternary(t) => {
            check_expr(source, t.condition, defs, out);
            if let Some(then_expr) = t.then_expr {
                check_expr(source, then_expr, defs, out);
            }
            check_expr(source, t.else_expr, defs, out);
        }
        _ => {}
    }
}

fn simple_ident_name<'a>(expr: &'a Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(name.to_string()),
        _ => None,
    }
}

// ── Return-type checking ──────────────────────────────────────────────────────

fn check_return_types_in_body<'a>(
    source: &str,
    body: &[Stmt<'a, 'a>],
    ret_type_str: &str,
    out: &mut Vec<Diagnostic>,
) {
    let is_nullable = ret_type_str.starts_with('?') || ret_type_str == "mixed";
    let base_type = ret_type_str.trim_start_matches('?');
    let is_void = base_type == "void" || base_type == "never";

    for stmt in body {
        match &stmt.kind {
            StmtKind::Return(Some(expr)) => {
                if is_void {
                    let (sl, sc) = offset_to_position(source, expr.span.start);
                    out.push(Diagnostic {
                        start_line: sl,
                        start_char: sc,
                        end_line: sl,
                        end_char: sc + 6,
                        severity: Severity::Warning,
                        message: "Returning a value from a void function".to_string(),
                        related: vec![],
                    });
                } else if let Some(msg) = literal_return_conflict(expr, base_type, is_nullable) {
                    let (sl, sc) = offset_to_position(source, expr.span.start);
                    let (el, ec) = offset_to_position(source, expr.span.end);
                    out.push(Diagnostic {
                        start_line: sl,
                        start_char: sc,
                        end_line: el,
                        end_char: ec,
                        severity: Severity::Warning,
                        message: msg,
                        related: vec![],
                    });
                }
            }
            StmtKind::If(i) => {
                check_return_types_in_body(
                    source,
                    std::slice::from_ref(i.then_branch),
                    ret_type_str,
                    out,
                );
                for b in i.elseif_branches.iter() {
                    check_return_types_in_body(
                        source,
                        std::slice::from_ref(&b.body),
                        ret_type_str,
                        out,
                    );
                }
                if let Some(b) = i.else_branch {
                    check_return_types_in_body(
                        source,
                        std::slice::from_ref(b),
                        ret_type_str,
                        out,
                    );
                }
            }
            StmtKind::While(w) => {
                check_return_types_in_body(
                    source,
                    std::slice::from_ref(&w.body),
                    ret_type_str,
                    out,
                );
            }
            StmtKind::Block(stmts) => {
                check_return_types_in_body(source, stmts, ret_type_str, out);
            }
            _ => {}
        }
    }
}

fn literal_return_conflict<'a>(
    expr: &Expr<'a, 'a>,
    base_type: &str,
    is_nullable: bool,
) -> Option<String> {
    match &expr.kind {
        ExprKind::Null => {
            if !is_nullable {
                Some(format!(
                    "Cannot return null from non-nullable function with return type '{base_type}'"
                ))
            } else {
                None
            }
        }
        ExprKind::String(_) => {
            if matches!(base_type, "int" | "integer" | "float" | "double" | "bool" | "boolean" | "array") {
                Some(format!(
                    "Returning string literal from function declared to return '{base_type}'"
                ))
            } else {
                None
            }
        }
        ExprKind::Int(_) => {
            if matches!(base_type, "string" | "bool" | "boolean" | "array") {
                Some(format!(
                    "Returning int literal from function declared to return '{base_type}'"
                ))
            } else {
                None
            }
        }
        ExprKind::Float(_) => {
            if matches!(base_type, "string" | "bool" | "boolean" | "array" | "int" | "integer") {
                Some(format!(
                    "Returning float literal from function declared to return '{base_type}'"
                ))
            } else {
                None
            }
        }
        ExprKind::Bool(_) => {
            if matches!(base_type, "string" | "int" | "integer" | "float" | "double" | "array") {
                Some(format!(
                    "Returning bool literal from function declared to return '{base_type}'"
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

// ── Diagnostic constructors ───────────────────────────────────────────────────

fn span_range(source: &str, span: Span, name: &str) -> (u32, u32, u32, u32) {
    let (sl, sc) = offset_to_position(source, span.start);
    (sl, sc, sl, sc + name.len() as u32)
}

fn undefined_diag(source: &str, span: Span, name: &str) -> Diagnostic {
    let (sl, sc, el, ec) = span_range(source, span, name);
    Diagnostic {
        start_line: sl,
        start_char: sc,
        end_line: el,
        end_char: ec,
        severity: Severity::Hint,
        message: format!("Undefined: {name}"),
        related: vec![],
    }
}

fn arity_diag(
    source: &str,
    span: Span,
    name: &str,
    required: usize,
    max: usize,
    got: usize,
    decl: Option<DeclLoc>,
) -> Diagnostic {
    let (sl, sc, el, ec) = span_range(source, span, name);
    let msg = if got < required {
        format!("Too few arguments to {name}: expected at least {required}, got {got}")
    } else {
        format!("Too many arguments to {name}: expected at most {max}, got {got}")
    };
    // If we know the declaration location (same-file function), add it as related info.
    let related = decl
        .map(|d| vec![(d.line, d.char, d.line, d.char + name.len() as u32, format!("{name} declared here"))])
        .unwrap_or_default();
    Diagnostic {
        start_line: sl,
        start_char: sc,
        end_line: el,
        end_char: ec,
        severity: Severity::Warning,
        message: msg,
        related,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bumpalo::Bump;

    fn parse_and_analyze(src: &str) -> Vec<Diagnostic> {
        let arena = Bump::new();
        let result = php_rs_parser::parse(&arena, src);
        let stmts: &[php_ast::Stmt<'_, '_>] = result.program.stmts.as_ref();
        analyze(src, stmts, &[(src, stmts)])
    }

    #[test]
    fn arity_error_for_too_few_args() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1);";
        let diags = parse_and_analyze(src);
        let arity_diags: Vec<_> = diags.iter().filter(|d| d.message.contains("Too few")).collect();
        assert!(!arity_diags.is_empty(), "expected arity diagnostic");
        // The related info should point to the function declaration.
        assert!(!arity_diags[0].related.is_empty(), "expected related info for same-file function");
        assert!(arity_diags[0].related[0].4.contains("declared here"), "related message should say 'declared here'");
    }

    #[test]
    fn arity_error_includes_declared_here_location() {
        let src = "<?php\nfunction greet(string $name): void {}\ngreet();";
        let diags = parse_and_analyze(src);
        let arity = diags.iter().find(|d| d.message.contains("greet")).unwrap();
        // The related location should point to line 1 (0-based) where `function greet` is declared.
        assert!(!arity.related.is_empty());
        assert_eq!(arity.related[0].0, 1, "declaration should be on line 1");
    }
}
