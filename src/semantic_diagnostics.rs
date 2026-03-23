/// Semantic diagnostics: undefined function/class calls and argument-count mismatches.
///
/// These complement the syntax diagnostics from the parser (which only report
/// parse errors).  Semantic diagnostics require a two-pass approach:
///   1. Collect definitions (functions, classes, methods) with arity.
///   2. Walk the AST for call sites and flag mismatches.
use std::collections::HashMap;
use std::sync::Arc;

use php_parser_rs::lexer::token::Span;
use php_parser_rs::parser::ast::{
    arguments::Argument,
    classes::ClassMember,
    identifiers::Identifier as AstIdentifier,
    namespaces::NamespaceStatement,
    Expression, Statement,
};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range, Url};

use crate::diagnostics::span_to_position;

/// Arity bounds for a callable.
#[derive(Debug, Clone, Copy)]
struct Arity {
    required: usize,
    max: usize, // usize::MAX for variadic
}

type DefMap = HashMap<String, Arity>;

/// Run semantic checks over `ast` and all `other_docs`, producing diagnostics
/// for the file at `uri`.  Only `uri`'s AST is checked for call sites; all
/// docs contribute to the definition map.
pub fn semantic_diagnostics(
    _uri: &Url,
    ast: &[Statement],
    other_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Vec<Diagnostic> {
    let mut defs: DefMap = HashMap::new();

    // Collect definitions from current file
    collect_defs(ast, &mut defs);

    // Collect definitions from other indexed files (cross-file calls)
    for (_, other_ast) in other_docs {
        collect_defs(other_ast, &mut defs);
    }

    let mut diagnostics = Vec::new();
    check_stmts(ast, &defs, &mut diagnostics);
    diagnostics
}

// ── Definition collection ─────────────────────────────────────────────────────

fn collect_defs(stmts: &[Statement], defs: &mut DefMap) {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) => {
                let arity = params_arity(&f.parameters);
                defs.insert(f.name.value.to_string(), arity);
            }
            Statement::Class(c) => {
                defs.insert(c.name.value.to_string(), Arity { required: 0, max: 0 });
                for member in &c.body.members {
                    match member {
                        ClassMember::ConcreteMethod(m) => {
                            let arity = params_arity(&m.parameters);
                            // Store as ClassName::method for scoped lookup
                            let key = format!("{}::{}", c.name.value, m.name.value);
                            defs.insert(key, arity);
                            // Also store unqualified so simple name lookups work
                            defs.insert(m.name.value.to_string(), arity);
                        }
                        ClassMember::AbstractMethod(m) => {
                            let arity = params_arity(&m.parameters);
                            defs.insert(m.name.value.to_string(), arity);
                        }
                        _ => {}
                    }
                }
            }
            Statement::Interface(i) => {
                defs.insert(i.name.value.to_string(), Arity { required: 0, max: 0 });
            }
            Statement::Trait(t) => {
                defs.insert(t.name.value.to_string(), Arity { required: 0, max: 0 });
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_defs(inner, defs);
            }
            _ => {}
        }
    }
}

fn params_arity(
    params: &php_parser_rs::parser::ast::functions::FunctionParameterList,
) -> Arity {
    let mut required = 0usize;
    let mut is_variadic = false;
    for p in params.parameters.iter() {
        if p.ellipsis.is_some() {
            is_variadic = true;
        } else if p.default.is_none() {
            required += 1;
        }
    }
    let total = params.parameters.iter().count();
    Arity {
        required,
        max: if is_variadic { usize::MAX } else { total },
    }
}

// ── Call-site checking ────────────────────────────────────────────────────────

fn check_stmts(stmts: &[Statement], defs: &DefMap, out: &mut Vec<Diagnostic>) {
    for stmt in stmts {
        check_stmt(stmt, defs, out);
    }
}

fn check_stmt(stmt: &Statement, defs: &DefMap, out: &mut Vec<Diagnostic>) {
    match stmt {
        Statement::Expression(e) => check_expr(&e.expression, defs, out),
        Statement::Return(r) => {
            if let Some(v) = &r.value {
                check_expr(v, defs, out);
            }
        }
        Statement::Echo(e) => {
            for expr in &e.values {
                check_expr(expr, defs, out);
            }
        }
        Statement::Function(f) => check_stmts(&f.body.statements, defs, out),
        Statement::Class(c) => {
            for member in &c.body.members {
                if let ClassMember::ConcreteMethod(m) = member {
                    check_stmts(&m.body.statements, defs, out);
                }
            }
        }
        Statement::Namespace(ns) => {
            let inner = match ns {
                NamespaceStatement::Unbraced(u) => &u.statements[..],
                NamespaceStatement::Braced(b) => &b.body.statements[..],
            };
            check_stmts(inner, defs, out);
        }
        _ => {}
    }
}

fn check_expr(expr: &Expression, defs: &DefMap, out: &mut Vec<Diagnostic>) {
    match expr {
        Expression::FunctionCall(f) => {
            if let Some(name) = simple_ident_name(&f.target) {
                let arg_count = f.arguments.arguments.len();
                if let Some(arity) = defs.get(&name) {
                    if arg_count < arity.required || arg_count > arity.max {
                        let span = span_of_expr(&f.target);
                        out.push(arity_diagnostic(span, &name, arity.required, arity.max, arg_count));
                    }
                } else {
                    // Undefined function — warn, not error (may be built-in)
                    let span = span_of_expr(&f.target);
                    if let Some(span) = span {
                        out.push(undefined_diagnostic(span, &name));
                    }
                }
            }
            for arg in &f.arguments.arguments {
                check_expr(arg_value(arg), defs, out);
            }
        }
        Expression::MethodCall(m) => {
            check_expr(&m.target, defs, out);
            for arg in &m.arguments.arguments {
                check_expr(arg_value(arg), defs, out);
            }
        }
        Expression::New(n) => {
            if let Some(name) = simple_ident_name(&n.target) {
                if !defs.contains_key(&name) {
                    let span = span_of_expr(&n.target);
                    if let Some(span) = span {
                        out.push(undefined_diagnostic(span, &name));
                    }
                }
            }
            if let Some(args) = &n.arguments {
                for arg in &args.arguments {
                    check_expr(arg_value(arg), defs, out);
                }
            }
        }
        Expression::AssignmentOperation(a) => {
            check_expr(a.left(), defs, out);
            check_expr(a.right(), defs, out);
        }
        Expression::Parenthesized(p) => check_expr(&p.expr, defs, out),
        Expression::Ternary(t) => {
            check_expr(&t.condition, defs, out);
            check_expr(&t.then, defs, out);
            check_expr(&t.r#else, defs, out);
        }
        _ => {}
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn simple_ident_name(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) => {
            Some(si.value.to_string())
        }
        _ => None,
    }
}

fn span_of_expr(expr: &Expression) -> Option<Span> {
    match expr {
        Expression::Identifier(AstIdentifier::SimpleIdentifier(si)) => Some(si.span),
        _ => None,
    }
}

fn arg_value(arg: &Argument) -> &Expression {
    match arg {
        Argument::Positional(p) => &p.value,
        Argument::Named(n) => &n.value,
    }
}

fn span_to_range(span: Span, name: &str) -> Range {
    let start = span_to_position(&span);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + name.len() as u32,
        },
    }
}

fn arity_diagnostic(
    span: Option<Span>,
    name: &str,
    required: usize,
    max: usize,
    got: usize,
) -> Diagnostic {
    let range = span
        .map(|s| span_to_range(s, name))
        .unwrap_or_default();
    let msg = if got < required {
        format!(
            "Too few arguments to {name}: expected at least {required}, got {got}"
        )
    } else {
        format!(
            "Too many arguments to {name}: expected at most {max}, got {got}"
        )
    };
    Diagnostic {
        range,
        severity: Some(DiagnosticSeverity::WARNING),
        source: Some("php-lsp".to_string()),
        message: msg,
        ..Default::default()
    }
}

fn undefined_diagnostic(span: Span, name: &str) -> Diagnostic {
    Diagnostic {
        range: span_to_range(span, name),
        severity: Some(DiagnosticSeverity::HINT),
        source: Some("php-lsp".to_string()),
        message: format!("Undefined: {name}"),
        ..Default::default()
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

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    fn run(src: &str) -> Vec<Diagnostic> {
        let ast = parse_ast(src);
        semantic_diagnostics(&uri(), &ast, &[])
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
        let ast = parse_ast("<?php\n$x = new MyService();");
        let other_uri = Url::parse("file:///other.php").unwrap();
        let other_ast = Arc::new(parse_ast("<?php\nclass MyService {}"));
        let diags = semantic_diagnostics(&uri(), &ast, &[(other_uri, other_ast)]);
        let has_undefined = diags.iter().any(|d| d.message.contains("Undefined"));
        assert!(!has_undefined, "cross-file class should not be flagged");
    }

    #[test]
    fn optional_params_do_not_inflate_required_count() {
        let diags = run("<?php\nfunction connect(string $host, int $port = 3306): void {}\nconnect('localhost');");
        assert!(diags.is_empty(), "optional param should not require argument");
    }
}
