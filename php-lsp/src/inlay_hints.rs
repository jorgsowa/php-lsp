use std::collections::HashMap;

use php_ast::{ClassMemberKind, EnumMemberKind, Expr, ExprKind, NamespaceBody, Stmt, StmtKind};
use serde_json::json;
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range};

use crate::ast::{ParsedDoc, format_type_hint, offset_to_position};

struct FuncDef {
    params: Vec<String>,
    return_type: Option<String>,
}

/// Returns parameter-name inlay hints AND return-type hints for all
/// function/method declarations and calls in `doc`.
pub fn inlay_hints(source: &str, doc: &ParsedDoc, range: Range) -> Vec<InlayHint> {
    let defs = collect_defs(&doc.program().stmts);
    let mut hints = Vec::new();
    hints_in_stmts(source, &doc.program().stmts, &defs, range, &mut hints);
    hints
}

// === Definition collection ===

fn collect_defs(stmts: &[Stmt<'_, '_>]) -> HashMap<String, FuncDef> {
    let mut map = HashMap::new();
    collect_defs_stmts(stmts, &mut map);
    map
}

fn collect_defs_stmts(stmts: &[Stmt<'_, '_>], map: &mut HashMap<String, FuncDef>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let params = f.params.iter().map(|p| p.name.to_string()).collect();
                let return_type = f.return_type.as_ref().map(|t| format_type_hint(t));
                map.insert(
                    f.name.to_string(),
                    FuncDef {
                        params,
                        return_type,
                    },
                );
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let params: Vec<String> =
                            m.params.iter().map(|p| p.name.to_string()).collect();
                        let return_type = m.return_type.as_ref().map(|t| format_type_hint(t));
                        // Register __construct under the class name so `new ClassName(...)` gets hints.
                        if m.name == "__construct"
                            && let Some(class_name) = c.name
                        {
                            map.insert(
                                class_name.to_string(),
                                FuncDef {
                                    params: params.clone(),
                                    return_type: None,
                                },
                            );
                        }
                        map.insert(
                            m.name.to_string(),
                            FuncDef {
                                params,
                                return_type,
                            },
                        );
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let params: Vec<String> =
                            m.params.iter().map(|p| p.name.to_string()).collect();
                        let return_type = m.return_type.as_ref().map(|t| format_type_hint(t));
                        map.insert(
                            m.name.to_string(),
                            FuncDef {
                                params,
                                return_type,
                            },
                        );
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        let params: Vec<String> =
                            m.params.iter().map(|p| p.name.to_string()).collect();
                        let return_type = m.return_type.as_ref().map(|t| format_type_hint(t));
                        map.insert(
                            m.name.to_string(),
                            FuncDef {
                                params,
                                return_type,
                            },
                        );
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_defs_stmts(inner, map);
                }
            }
            // Register closure/arrow-function variables so `$fn(...)` call sites get hints.
            StmtKind::Expression(e) => {
                if let ExprKind::Assign(assign) = &e.kind
                    && let ExprKind::Variable(var_name) = &assign.target.kind
                {
                    let key = format!("${var_name}");
                    match &assign.value.kind {
                        ExprKind::Closure(c) => {
                            let params = c.params.iter().map(|p| p.name.to_string()).collect();
                            let return_type = c.return_type.as_ref().map(|t| format_type_hint(t));
                            map.insert(
                                key,
                                FuncDef {
                                    params,
                                    return_type,
                                },
                            );
                        }
                        ExprKind::ArrowFunction(a) => {
                            let params = a.params.iter().map(|p| p.name.to_string()).collect();
                            let return_type = a.return_type.as_ref().map(|t| format_type_hint(t));
                            map.insert(
                                key,
                                FuncDef {
                                    params,
                                    return_type,
                                },
                            );
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

// === AST walking ===

fn hints_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    defs: &HashMap<String, FuncDef>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    for stmt in stmts {
        hints_in_stmt(source, stmt, defs, range, out);
    }
}

fn hints_in_stmt(
    source: &str,
    stmt: &Stmt<'_, '_>,
    defs: &HashMap<String, FuncDef>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    match &stmt.kind {
        StmtKind::Expression(e) => hints_in_expr(source, e, defs, range, out),
        StmtKind::Return(Some(v)) => hints_in_expr(source, v, defs, range, out),
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                hints_in_expr(source, expr, defs, range, out);
            }
        }
        StmtKind::Function(f) => {
            hints_in_stmts(source, &f.body, defs, range, out);
        }
        StmtKind::Class(c) => {
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(source, body, defs, range, out);
                }
            }
        }
        StmtKind::Trait(t) => {
            for member in t.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(source, body, defs, range, out);
                }
            }
        }
        StmtKind::Enum(e) => {
            for member in e.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(source, body, defs, range, out);
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                hints_in_stmts(source, inner, defs, range, out);
            }
        }
        StmtKind::If(i) => {
            hints_in_expr(source, &i.condition, defs, range, out);
            hints_in_stmt(source, i.then_branch, defs, range, out);
            for ei in i.elseif_branches.iter() {
                hints_in_expr(source, &ei.condition, defs, range, out);
                hints_in_stmt(source, &ei.body, defs, range, out);
            }
            if let Some(e) = &i.else_branch {
                hints_in_stmt(source, e, defs, range, out);
            }
        }
        StmtKind::While(w) => {
            hints_in_expr(source, &w.condition, defs, range, out);
            hints_in_stmt(source, w.body, defs, range, out);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                hints_in_expr(source, e, defs, range, out);
            }
            for cond in f.condition.iter() {
                hints_in_expr(source, cond, defs, range, out);
            }
            for e in f.update.iter() {
                hints_in_expr(source, e, defs, range, out);
            }
            hints_in_stmt(source, f.body, defs, range, out);
        }
        StmtKind::Foreach(f) => {
            hints_in_expr(source, &f.expr, defs, range, out);
            hints_in_stmt(source, f.body, defs, range, out);
        }
        StmtKind::TryCatch(t) => {
            hints_in_stmts(source, &t.body, defs, range, out);
            for catch in t.catches.iter() {
                hints_in_stmts(source, &catch.body, defs, range, out);
            }
            if let Some(finally) = &t.finally {
                hints_in_stmts(source, finally, defs, range, out);
            }
        }
        StmtKind::Block(stmts) => hints_in_stmts(source, stmts, defs, range, out),
        _ => {}
    }
}

fn hints_in_expr(
    source: &str,
    expr: &Expr<'_, '_>,
    defs: &HashMap<String, FuncDef>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    match &expr.kind {
        ExprKind::FunctionCall(f) => {
            // Look up by identifier name or by variable name (for closure vars like `$fn(...)`).
            let key: Option<String> = ident_name(f.name).map(|n| n.to_string()).or_else(|| {
                if let ExprKind::Variable(n) = &f.name.kind {
                    Some(format!("${n}"))
                } else {
                    None
                }
            });
            if let Some(k) = key
                && let Some(def) = defs.get(&k)
            {
                emit_param_hints(source, &f.args, &def.params, &k, range, out);
            }
            hints_in_expr(source, f.name, defs, range, out);
            for arg in f.args.iter() {
                hints_in_expr(source, &arg.value, defs, range, out);
            }
        }
        ExprKind::MethodCall(m) => {
            if let Some(name) = ident_name(m.method)
                && let Some(def) = defs.get(name)
            {
                emit_param_hints(source, &m.args, &def.params, name, range, out);
            }
            hints_in_expr(source, m.object, defs, range, out);
            for arg in m.args.iter() {
                hints_in_expr(source, &arg.value, defs, range, out);
            }
        }
        ExprKind::New(n) => {
            if let Some(class_name) = ident_name(n.class)
                && let Some(def) = defs.get(class_name)
            {
                emit_param_hints(source, &n.args, &def.params, class_name, range, out);
            }
            for arg in n.args.iter() {
                hints_in_expr(source, &arg.value, defs, range, out);
            }
        }
        ExprKind::Assign(a) => {
            // Emit return-type hint after a function call on the RHS
            emit_return_type_hint(source, a.value, defs, range, out);
            hints_in_expr(source, a.target, defs, range, out);
            hints_in_expr(source, a.value, defs, range, out);
        }
        // Walk into closure bodies so nested function calls get hints.
        ExprKind::Closure(c) => {
            hints_in_stmts(source, &c.body, defs, range, out);
        }
        // Walk into arrow function bodies.
        ExprKind::ArrowFunction(a) => {
            hints_in_expr(source, a.body, defs, range, out);
        }
        ExprKind::Parenthesized(e) => hints_in_expr(source, e, defs, range, out),
        ExprKind::Ternary(t) => {
            hints_in_expr(source, t.condition, defs, range, out);
            if let Some(then_expr) = t.then_expr {
                hints_in_expr(source, then_expr, defs, range, out);
            }
            hints_in_expr(source, t.else_expr, defs, range, out);
        }
        ExprKind::NullCoalesce(n) => {
            hints_in_expr(source, n.left, defs, range, out);
            hints_in_expr(source, n.right, defs, range, out);
        }
        ExprKind::Binary(b) => {
            hints_in_expr(source, b.left, defs, range, out);
            hints_in_expr(source, b.right, defs, range, out);
        }
        _ => {}
    }
}

fn emit_param_hints(
    source: &str,
    args: &[php_ast::Arg<'_, '_>],
    params: &[String],
    func_name: &str,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    for (i, arg) in args.iter().enumerate() {
        // Skip named arguments (they already have the label in source)
        if arg.name.is_some() {
            continue;
        }
        if let Some(param) = params.get(i) {
            let pos = offset_to_position(source, arg.span.start);
            if pos_in_range(pos, range) {
                out.push(make_param_hint(pos, param, func_name));
            }
        }
    }
}

fn emit_return_type_hint(
    source: &str,
    expr: &Expr<'_, '_>,
    defs: &HashMap<String, FuncDef>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    let name = match &expr.kind {
        ExprKind::FunctionCall(f) => ident_name(f.name),
        ExprKind::MethodCall(m) => ident_name(m.method),
        _ => return,
    };
    if let Some(name) = name
        && let Some(def) = defs.get(name)
        && let Some(ret_type) = &def.return_type
    {
        if ret_type == "void" {
            return;
        }
        let pos = offset_to_position(source, expr.span.end);
        if pos_in_range(pos, range) {
            out.push(make_return_hint(pos, ret_type, name));
        }
    }
}

fn ident_name<'a>(expr: &'a Expr<'_, '_>) -> Option<&'a str> {
    if let ExprKind::Identifier(name) = &expr.kind {
        Some(name.as_ref())
    } else {
        None
    }
}

fn make_param_hint(position: Position, param_name: &str, func_name: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!("{}:", param_name)),
        kind: Some(InlayHintKind::PARAMETER),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: Some(true),
        data: Some(json!({"php_lsp_fn": func_name})),
    }
}

fn make_return_hint(position: Position, ret_type: &str, func_name: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {ret_type}")),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: None,
        data: Some(json!({"php_lsp_fn": func_name})),
    }
}

fn pos_in_range(pos: Position, range: Range) -> bool {
    if pos.line < range.start.line || pos.line > range.end.line {
        return false;
    }
    if pos.line == range.start.line && pos.character < range.start.character {
        return false;
    }
    if pos.line == range.end.line && pos.character > range.end.character {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    fn full_range() -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: u32::MAX,
                character: u32::MAX,
            },
        }
    }

    fn label_str(hint: &InlayHint) -> &str {
        match &hint.label {
            InlayHintLabel::String(s) => s.as_str(),
            InlayHintLabel::LabelParts(_) => "",
        }
    }

    #[test]
    fn emits_hint_for_single_param_call() {
        let src = "<?php\nfunction greet(string $name): void {}\ngreet('Alice');";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "name:");
    }

    #[test]
    fn emits_hints_for_multiple_params() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert_eq!(hints.len(), 2);
        assert_eq!(label_str(&hints[0]), "a:");
        assert_eq!(label_str(&hints[1]), "b:");
    }

    #[test]
    fn no_hints_for_unknown_function() {
        let src = "<?php\nunknownFn(1, 2);";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert!(hints.is_empty());
    }

    #[test]
    fn no_hints_for_zero_param_call() {
        let src = "<?php\nfunction init(): void {}\ninit();";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert!(hints.is_empty());
    }

    #[test]
    fn skips_named_arguments() {
        let src = "<?php\nfunction greet(string $name): void {}\ngreet(name: 'Alice');";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert!(hints.is_empty());
    }

    #[test]
    fn hint_kind_is_parameter() {
        let src = "<?php\nfunction f(int $x): void {}\nf(1);";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert_eq!(hints[0].kind, Some(InlayHintKind::PARAMETER));
    }

    #[test]
    fn hint_position_is_at_argument_start() {
        let src = "<?php\nfunction greet(string $name): void {}\ngreet('Alice');";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(
            hints[0].position,
            Position {
                line: 2,
                character: 6
            }
        );
    }

    #[test]
    fn hint_positions_for_multiple_args() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert_eq!(hints.len(), 2);
        assert_eq!(
            hints[0].position,
            Position {
                line: 2,
                character: 4
            }
        );
        assert_eq!(
            hints[1].position,
            Position {
                line: 2,
                character: 7
            }
        );
    }

    #[test]
    fn fewer_args_than_params_emits_hints_for_provided_args_only() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1);";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "a:");
    }

    #[test]
    fn more_args_than_params_emits_hints_only_for_known_params() {
        let src = "<?php\nfunction f(int $x): void {}\nf(1, 2, 3);";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "x:");
    }

    #[test]
    fn return_type_hint_for_assignment() {
        let src = "<?php\nfunction make(): string { return 'x'; }\n$s = make();";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let ret_hint = hints.iter().find(|h| label_str(h) == ": string");
        assert!(ret_hint.is_some(), "expected ': string' return type hint");
    }

    #[test]
    fn no_return_type_hint_for_void() {
        let src = "<?php\nfunction init(): void {}\n$x = init();";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let ret_hint = hints.iter().find(|h| label_str(h).starts_with(": "));
        assert!(
            ret_hint.is_none(),
            "void return type should not produce a hint"
        );
    }

    #[test]
    fn hints_for_function_inside_namespace() {
        let src = "<?php\nnamespace App;\nfunction greet(string $name): void {}\ngreet('Alice');";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        assert_eq!(hints.len(), 1);
        assert_eq!(label_str(&hints[0]), "name:");
    }

    #[test]
    fn closure_variable_call_gets_param_hints() {
        let src =
            "<?php\n$greet = function(string $name, int $times): void {};\n$greet('Alice', 3);";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(param_hints.contains(&"name:"), "missing 'name:' hint");
        assert!(param_hints.contains(&"times:"), "missing 'times:' hint");
    }

    #[test]
    fn arrow_function_variable_call_gets_param_hints() {
        let src = "<?php\n$double = fn(int $n): int => $n * 2;\n$result = $double(5);";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(param_hints.contains(&"n:"), "missing 'n:' param hint");
    }

    #[test]
    fn function_call_inside_closure_body_gets_hints() {
        let src = "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\n$fn = function() { add(1, 2); };";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(
            param_hints.contains(&"a:"),
            "missing 'a:' hint inside closure body"
        );
        assert!(
            param_hints.contains(&"b:"),
            "missing 'b:' hint inside closure body"
        );
    }

    #[test]
    fn hints_outside_range_excluded() {
        // The function call is on line 2; requesting a range that ends on line 1
        // should return zero hints.
        let src = "<?php\nfunction greet(string $name): void {}\ngreet('Alice');";
        let d = doc(src);
        // Range covers only lines 0-1 (the declaration), excluding line 2 (the call).
        let narrow_range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 1,
                character: u32::MAX,
            },
        };
        let hints = inlay_hints(src, &d, narrow_range);
        assert!(
            hints.is_empty(),
            "hints on line 2 should be excluded when range ends at line 1, got: {:?}",
            hints
        );
    }

    #[test]
    fn method_call_gets_param_hints() {
        // $obj->method($arg) where method has a named param should get a param hint.
        let src = concat!(
            "<?php\n",
            "class Greeter {\n",
            "    public function sayHello(string $name): void {}\n",
            "}\n",
            "$g = new Greeter();\n",
            "$g->sayHello('World');\n",
        );
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(
            param_hints.contains(&"name:"),
            "expected 'name:' param hint for method call, got: {:?}",
            param_hints
        );
        assert_eq!(
            param_hints.len(),
            1,
            "expected exactly 1 param hint, got: {:?}",
            param_hints
        );
    }

    #[test]
    fn new_expression_gets_constructor_param_hints() {
        // `new Point(1, 2)` should emit `x:` and `y:` hints from __construct.
        let src = concat!(
            "<?php\n",
            "class Point {\n",
            "    public function __construct(int $x, int $y) {}\n",
            "}\n",
            "$p = new Point(1, 2);\n",
        );
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(
            param_hints.contains(&"x:"),
            "expected 'x:' hint for __construct, got: {:?}",
            param_hints
        );
        assert!(
            param_hints.contains(&"y:"),
            "expected 'y:' hint for __construct, got: {:?}",
            param_hints
        );
        assert_eq!(
            param_hints.len(),
            2,
            "expected exactly 2 constructor param hints, got: {:?}",
            param_hints
        );
    }

    #[test]
    fn trait_method_call_gets_param_hints() {
        // Methods defined in traits should produce param hints.
        let src = concat!(
            "<?php\n",
            "trait Logger {\n",
            "    public function log(string $msg, int $level): void {}\n",
            "}\n",
            "log('hello', 3);\n",
        );
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(
            param_hints.contains(&"msg:"),
            "expected 'msg:' hint for trait method, got: {:?}",
            param_hints
        );
        assert!(
            param_hints.contains(&"level:"),
            "expected 'level:' hint, got: {:?}",
            param_hints
        );
    }

    #[test]
    fn for_loop_init_and_update_get_hints() {
        // Function calls in `for` init and update expressions should produce param hints.
        let src = concat!(
            "<?php\n",
            "function tick(int $n): void {}\n",
            "for (tick(1); $i < 10; tick(2)) {}\n",
        );
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert_eq!(
            param_hints.len(),
            2,
            "expected 2 'n:' hints (init + update), got: {:?}",
            param_hints
        );
        assert!(
            param_hints.iter().all(|&l| l == "n:"),
            "all hints should be 'n:', got: {:?}",
            param_hints
        );
    }

    #[test]
    fn new_expression_no_hints_without_constructor() {
        // `new Foo()` where Foo has no __construct should produce no param hints.
        let src = "<?php\nclass Foo {}\n$f = new Foo();\n";
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(
            param_hints.is_empty(),
            "expected no hints for class without constructor, got: {:?}",
            param_hints
        );
    }

    #[test]
    fn calls_inside_trait_method_body_get_hints() {
        let src = concat!(
            "<?php\n",
            "function write(string $msg): void {}\n",
            "trait Logger {\n",
            "    public function log(): void { write('hello'); }\n",
            "}\n",
        );
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(
            param_hints.contains(&"msg:"),
            "expected 'msg:' hint for call inside trait method body, got: {:?}",
            param_hints
        );
    }

    #[test]
    fn calls_inside_enum_method_body_get_hints() {
        let src = concat!(
            "<?php\n",
            "function write(string $msg): void {}\n",
            "enum Status {\n",
            "    case Active;\n",
            "    public function log(): void { write('hello'); }\n",
            "}\n",
        );
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(
            param_hints.contains(&"msg:"),
            "expected 'msg:' hint for call inside enum method body, got: {:?}",
            param_hints
        );
    }

    #[test]
    fn enum_method_call_gets_param_hints() {
        let src = concat!(
            "<?php\n",
            "enum Status {\n",
            "    case Active;\n",
            "    public function label(string $prefix, int $pad): string { return ''; }\n",
            "}\n",
            "label('x', 2);\n",
        );
        let d = doc(src);
        let hints = inlay_hints(src, &d, full_range());
        let param_hints: Vec<&str> = hints
            .iter()
            .filter(|h| h.kind == Some(InlayHintKind::PARAMETER))
            .map(|h| label_str(h))
            .collect();
        assert!(
            param_hints.contains(&"prefix:"),
            "expected 'prefix:' hint for enum method, got: {:?}",
            param_hints
        );
        assert!(
            param_hints.contains(&"pad:"),
            "expected 'pad:' hint, got: {:?}",
            param_hints
        );
    }
}
