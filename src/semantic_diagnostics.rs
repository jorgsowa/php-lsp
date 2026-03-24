/// Semantic diagnostics: undefined function/class calls and argument-count mismatches.
use std::collections::HashMap;
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
        StmtKind::Function(f) => check_stmts(source, &f.body, defs, out),
        StmtKind::Class(c) => {
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if let Some(body) = &m.body {
                        check_stmts(source, body, defs, out);
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
    fn optional_params_do_not_inflate_required_count() {
        let diags = run("<?php\nfunction connect(string $host, int $port = 3306): void {}\nconnect('localhost');");
        assert!(diags.is_empty(), "optional param should not require argument");
    }
}
