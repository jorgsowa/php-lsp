/// Semantic diagnostics: undefined function/class calls, argument-count mismatches,
/// and undefined variable usage within function/method bodies.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use php_ast::{ClassMemberKind, ExprKind, Expr, NamespaceBody, Span, Stmt, StmtKind};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range, Url};

use crate::ast::{offset_to_position, ParsedDoc};

/// Arity bounds for a callable.
#[derive(Debug, Clone, Copy)]
struct Arity {
    required: usize,
    max: usize, // usize::MAX for variadic
}

type DefMap = HashMap<String, Arity>;

/// Run semantic checks over the current doc and all `other_docs`, producing
/// diagnostics for the file at `uri`.
pub fn semantic_diagnostics(
    _uri: &Url,
    doc: &ParsedDoc,
    other_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<Diagnostic> {
    let source = doc.source();
    let mut defs: DefMap = HashMap::new();

    collect_defs(&doc.program().stmts, &mut defs);
    for (_, other_doc) in other_docs {
        collect_defs(&other_doc.program().stmts, &mut defs);
    }

    let mut diagnostics = Vec::new();
    check_stmts(source, &doc.program().stmts, &defs, &mut diagnostics);
    diagnostics
}

fn collect_defs(stmts: &[Stmt<'_, '_>], defs: &mut DefMap) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let arity = params_arity_slice(&f.params);
                defs.insert(f.name.to_string(), arity);
            }
            StmtKind::Class(c) => {
                let class_name = c.name.unwrap_or("");
                defs.insert(class_name.to_string(), Arity { required: 0, max: 0 });
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let arity = params_arity_slice(&m.params);
                        if !class_name.is_empty() {
                            defs.insert(format!("{}::{}", class_name, m.name), arity);
                        }
                        defs.insert(m.name.to_string(), arity);
                    }
                }
            }
            StmtKind::Interface(i) => {
                defs.insert(i.name.to_string(), Arity { required: 0, max: 0 });
            }
            StmtKind::Trait(t) => {
                defs.insert(t.name.to_string(), Arity { required: 0, max: 0 });
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_defs(inner, defs);
                }
            }
            _ => {}
        }
    }
}

fn params_arity_slice(params: &[php_ast::Param<'_, '_>]) -> Arity {
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

fn check_stmts(source: &str, stmts: &[Stmt<'_, '_>], defs: &DefMap, out: &mut Vec<Diagnostic>) {
    for stmt in stmts {
        check_stmt(source, stmt, defs, out);
    }
}

fn check_stmt(source: &str, stmt: &Stmt<'_, '_>, defs: &DefMap, out: &mut Vec<Diagnostic>) {
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
        }
        StmtKind::Class(c) => {
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if let Some(body) = &m.body {
                        check_stmts(source, body, defs, out);
                        check_function_vars(source, m.params.as_ref(), body, out);
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

// ── Undefined variable detection ─────────────────────────────────────────────

const SUPERGLOBALS: &[&str] = &[
    "GLOBALS", "_GET", "_POST", "_SESSION", "_SERVER", "_ENV",
    "_COOKIE", "_FILES", "_REQUEST",
];

/// Check for uses of variables that are never assigned anywhere in the function.
fn check_function_vars(
    source: &str,
    params: &[php_ast::Param<'_, '_>],
    body: &[Stmt<'_, '_>],
    out: &mut Vec<Diagnostic>,
) {
    // Bail if the function uses `global` or `extract` — too dynamic to analyse.
    if uses_global_or_extract(body) { return; }

    let mut defined: HashSet<String> = HashSet::new();
    defined.insert("this".to_string());
    for sg in SUPERGLOBALS { defined.insert(sg.to_string()); }
    for p in params { defined.insert(p.name.to_string()); }

    // First pass: collect every variable that appears as an assignment target,
    // foreach binding, or catch binding anywhere in the body.
    collect_defined_in_stmts(body, &mut defined);

    // Second pass: report uses of variables not in `defined`.
    check_var_uses_in_stmts(source, body, &defined, out);
}

fn uses_global_or_extract(stmts: &[Stmt<'_, '_>]) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Global(_) | StmtKind::StaticVar(_) => return true,
            StmtKind::Expression(e) => {
                if let ExprKind::FunctionCall(f) = &e.kind {
                    if let Some(name) = simple_fn_name(f.name) {
                        if name == "extract" || name == "compact" { return true; }
                    }
                }
            }
            StmtKind::If(i) => {
                if uses_global_or_extract(std::slice::from_ref(i.then_branch))
                    || i.elseif_branches.iter().any(|b| uses_global_or_extract(std::slice::from_ref(&b.body)))
                    || i.else_branch.map_or(false, |b| uses_global_or_extract(std::slice::from_ref(b)))
                { return true; }
            }
            _ => {}
        }
    }
    false
}

fn simple_fn_name<'a>(expr: &'a Expr<'_, '_>) -> Option<&'a str> {
    if let ExprKind::Identifier(n) = &expr.kind { Some(n.as_ref()) } else { None }
}

fn collect_defined_in_stmts(stmts: &[Stmt<'_, '_>], defined: &mut HashSet<String>) {
    for stmt in stmts {
        collect_defined_in_stmt(stmt, defined);
    }
}

fn collect_defined_in_stmt(stmt: &Stmt<'_, '_>, defined: &mut HashSet<String>) {
    match &stmt.kind {
        StmtKind::Expression(e) => collect_defined_in_expr(e, defined),
        StmtKind::Return(Some(e)) => collect_defined_in_expr(e, defined),
        StmtKind::Echo(exprs) => { for e in exprs.iter() { collect_defined_in_expr(e, defined); } }
        StmtKind::If(i) => {
            collect_defined_in_expr(&i.condition, defined);
            collect_defined_in_stmt(i.then_branch, defined);
            for b in i.elseif_branches.iter() { collect_defined_in_stmt(&b.body, defined); }
            if let Some(b) = i.else_branch { collect_defined_in_stmt(b, defined); }
        }
        StmtKind::While(w) => {
            collect_defined_in_expr(&w.condition, defined);
            collect_defined_in_stmt(&w.body, defined);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() { collect_defined_in_expr(e, defined); }
            for e in f.condition.iter() { collect_defined_in_expr(e, defined); }
            for e in f.update.iter() { collect_defined_in_expr(e, defined); }
            collect_defined_in_stmt(&f.body, defined);
        }
        StmtKind::Foreach(f) => {
            if let ExprKind::Variable(k) = &f.key.as_ref().map(|k| &k.kind).unwrap_or(&ExprKind::Null) {
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
                if let Some(v) = catch.var { defined.insert(v.to_string()); }
                collect_defined_in_stmts(&catch.body, defined);
            }
        }
        StmtKind::Block(stmts) => collect_defined_in_stmts(stmts, defined),
        _ => {}
    }
}

fn collect_defined_in_expr(expr: &Expr<'_, '_>, defined: &mut HashSet<String>) {
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

fn check_var_uses_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    defined: &HashSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    for stmt in stmts {
        check_var_uses_in_stmt(source, stmt, defined, out);
    }
}

fn check_var_uses_in_stmt(
    source: &str,
    stmt: &Stmt<'_, '_>,
    defined: &HashSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    match &stmt.kind {
        StmtKind::Expression(e) => check_var_uses_in_expr(source, e, defined, out),
        StmtKind::Return(Some(e)) => check_var_uses_in_expr(source, e, defined, out),
        StmtKind::Echo(exprs) => { for e in exprs.iter() { check_var_uses_in_expr(source, e, defined, out); } }
        StmtKind::If(i) => {
            check_var_uses_in_expr(source, &i.condition, defined, out);
            check_var_uses_in_stmt(source, i.then_branch, defined, out);
            for b in i.elseif_branches.iter() { check_var_uses_in_stmt(source, &b.body, defined, out); }
            if let Some(b) = i.else_branch { check_var_uses_in_stmt(source, b, defined, out); }
        }
        StmtKind::While(w) => {
            check_var_uses_in_expr(source, &w.condition, defined, out);
            check_var_uses_in_stmt(source, &w.body, defined, out);
        }
        StmtKind::For(f) => {
            for e in f.condition.iter() { check_var_uses_in_expr(source, e, defined, out); }
            for e in f.update.iter() { check_var_uses_in_expr(source, e, defined, out); }
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

fn check_var_uses_in_expr(
    source: &str,
    expr: &Expr<'_, '_>,
    defined: &HashSet<String>,
    out: &mut Vec<Diagnostic>,
) {
    match &expr.kind {
        ExprKind::Variable(name) => {
            let n = name.as_ref();
            if !defined.contains(n) {
                out.push(Diagnostic {
                    range: span_to_range(source, expr.span, &format!("${n}")),
                    severity: Some(DiagnosticSeverity::HINT),
                    source: Some("php-lsp".to_string()),
                    message: format!("Undefined variable: ${n}"),
                    ..Default::default()
                });
            }
        }
        ExprKind::Assign(a) => {
            // Only check the RHS; the LHS is a definition site.
            check_var_uses_in_expr(source, a.value, defined, out);
        }
        ExprKind::FunctionCall(f) => {
            for arg in f.args.iter() { check_var_uses_in_expr(source, &arg.value, defined, out); }
        }
        ExprKind::MethodCall(m) => {
            check_var_uses_in_expr(source, m.object, defined, out);
            for arg in m.args.iter() { check_var_uses_in_expr(source, &arg.value, defined, out); }
        }
        ExprKind::New(n) => {
            for arg in n.args.iter() { check_var_uses_in_expr(source, &arg.value, defined, out); }
        }
        ExprKind::Parenthesized(e) => check_var_uses_in_expr(source, e, defined, out),
        ExprKind::Ternary(t) => {
            check_var_uses_in_expr(source, t.condition, defined, out);
            if let Some(e) = t.then_expr { check_var_uses_in_expr(source, e, defined, out); }
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
            if let Some(idx) = a.index { check_var_uses_in_expr(source, idx, defined, out); }
        }
        _ => {}
    }
}

fn check_expr(source: &str, expr: &Expr<'_, '_>, defs: &DefMap, out: &mut Vec<Diagnostic>) {
    match &expr.kind {
        ExprKind::FunctionCall(f) => {
            if let Some(name) = simple_ident_name(f.name) {
                let arg_count = f.args.len();
                if let Some(arity) = defs.get(&name) {
                    if arg_count < arity.required || arg_count > arity.max {
                        out.push(arity_diagnostic(source, f.name.span, &name, arity.required, arity.max, arg_count));
                    }
                } else {
                    out.push(undefined_diagnostic(source, f.name.span, &name));
                }
            }
            for arg in f.args.iter() {
                check_expr(source, &arg.value, defs, out);
            }
        }
        ExprKind::MethodCall(m) => {
            check_expr(source, m.object, defs, out);
            for arg in m.args.iter() {
                check_expr(source, &arg.value, defs, out);
            }
        }
        ExprKind::New(n) => {
            if let Some(name) = simple_ident_name(n.class) {
                if !defs.contains_key(&name) {
                    out.push(undefined_diagnostic(source, n.class.span, &name));
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

fn simple_ident_name(expr: &Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(name.to_string()),
        _ => None,
    }
}

fn span_to_range(source: &str, span: Span, name: &str) -> Range {
    let start = offset_to_position(source, span.start);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + name.len() as u32,
        },
    }
}

fn arity_diagnostic(
    source: &str,
    span: Span,
    name: &str,
    required: usize,
    max: usize,
    got: usize,
) -> Diagnostic {
    let range = span_to_range(source, span, name);
    let msg = if got < required {
        format!("Too few arguments to {name}: expected at least {required}, got {got}")
    } else {
        format!("Too many arguments to {name}: expected at most {max}, got {got}")
    };
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        source: Some("php-lsp".to_string()),
        message: msg,
        ..Default::default()
    }
}

fn undefined_diagnostic(source: &str, span: Span, name: &str) -> Diagnostic {
    Diagnostic {
        range: span_to_range(source, span, name),
        severity: Some(DiagnosticSeverity::HINT),
        source: Some("php-lsp".to_string()),
        message: format!("Undefined: {name}"),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    fn run(src: &str) -> Vec<Diagnostic> {
        let doc = ParsedDoc::parse(src.to_string());
        semantic_diagnostics(&uri(), &doc, &[])
    }

    #[test]
    fn no_diagnostics_for_correct_call() {
        let diags = run("<?php\nfunction greet(string $name): void {}\ngreet('Alice');");
        assert!(diags.is_empty(), "unexpected: {:?}", diags);
    }

    #[test]
    fn detects_too_few_arguments() {
        let diags = run("<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1);");
        assert!(!diags.is_empty(), "expected arity diagnostic");
        assert!(diags[0].message.contains("Too few"));
    }

    #[test]
    fn detects_too_many_arguments() {
        let diags = run("<?php\nfunction f(int $x): void {}\nf(1, 2, 3);");
        assert!(!diags.is_empty(), "expected arity diagnostic");
        assert!(diags[0].message.contains("Too many"));
    }

    #[test]
    fn no_diagnostic_for_variadic_function() {
        let diags = run("<?php\nfunction log(string ...$msgs): void {}\nlog('a', 'b', 'c');");
        assert!(diags.is_empty(), "variadic should accept any count");
    }

    #[test]
    fn undefined_class_gives_hint() {
        let diags = run("<?php\n$x = new UnknownClass();");
        let has_undefined = diags.iter().any(|d| d.message.contains("Undefined"));
        assert!(has_undefined, "expected undefined hint: {:?}", diags);
    }

    #[test]
    fn cross_file_definition_suppresses_undefined() {
        let doc = ParsedDoc::parse("<?php\n$x = new MyService();".to_string());
        let other_uri = Url::parse("file:///other.php").unwrap();
        let other_doc = Arc::new(ParsedDoc::parse("<?php\nclass MyService {}".to_string()));
        let diags = semantic_diagnostics(&uri(), &doc, &[(other_uri, other_doc)]);
        let has_undefined = diags.iter().any(|d| d.message.contains("Undefined"));
        assert!(!has_undefined, "cross-file class should not be flagged");
    }

    #[test]
    fn undefined_variable_in_function_gives_hint() {
        let diags = run("<?php\nfunction foo() { echo $undefined; }");
        let has_undef = diags.iter().any(|d| d.message.contains("Undefined variable"));
        assert!(has_undef, "expected undefined variable hint: {:?}", diags);
    }

    #[test]
    fn defined_variable_in_function_gives_no_hint() {
        let diags = run("<?php\nfunction foo() { $x = 1; echo $x; }");
        let has_undef = diags.iter().any(|d| d.message.contains("Undefined variable"));
        assert!(!has_undef, "false positive: {:?}", diags);
    }

    #[test]
    fn function_param_is_defined() {
        let diags = run("<?php\nfunction foo($bar) { echo $bar; }");
        let has_undef = diags.iter().any(|d| d.message.contains("Undefined variable"));
        assert!(!has_undef, "param should be defined: {:?}", diags);
    }

    #[test]
    fn this_is_always_defined_in_method() {
        let diags = run("<?php\nclass Foo { public function bar() { return $this; } }");
        let has_undef = diags.iter().any(|d| d.message.contains("Undefined variable"));
        assert!(!has_undef, "$this should not be flagged: {:?}", diags);
    }

    #[test]
    fn global_suppresses_undefined_variable_check() {
        let diags = run("<?php\nfunction foo() { global $db; $db->query(); }");
        let has_undef = diags.iter().any(|d| d.message.contains("Undefined variable"));
        assert!(!has_undef, "global should suppress check: {:?}", diags);
    }

    #[test]
    fn optional_params_do_not_inflate_required_count() {
        let diags = run("<?php\nfunction connect(string $host, int $port = 3306): void {}\nconnect('localhost');");
        assert!(diags.is_empty(), "optional param should not require argument");
    }
}
