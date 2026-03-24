use std::collections::HashMap;

use php_ast::{ClassMemberKind, Expr, ExprKind, NamespaceBody, Stmt, StmtKind};
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
                        let params = m.params.iter().map(|p| p.name.to_string()).collect();
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
        StmtKind::Return(r) => {
            if let Some(v) = r {
                hints_in_expr(source, v, defs, range, out);
            }
        }
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
                if let ClassMemberKind::Method(m) = &member.kind {
                    if let Some(body) = &m.body {
                        hints_in_stmts(source, body, defs, range, out);
                    }
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
            for cond in f.condition.iter() {
                hints_in_expr(source, cond, defs, range, out);
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
            if let Some(name) = ident_name(f.name) {
                if let Some(def) = defs.get(name) {
                    emit_param_hints(source, &f.args, &def.params, range, out);
                }
            }
            hints_in_expr(source, f.name, defs, range, out);
            for arg in f.args.iter() {
                hints_in_expr(source, &arg.value, defs, range, out);
            }
        }
        ExprKind::MethodCall(m) => {
            if let Some(name) = ident_name(m.method) {
                if let Some(def) = defs.get(name) {
                    emit_param_hints(source, &m.args, &def.params, range, out);
                }
            }
            hints_in_expr(source, m.object, defs, range, out);
            for arg in m.args.iter() {
                hints_in_expr(source, &arg.value, defs, range, out);
            }
        }
        ExprKind::Assign(a) => {
            // Emit return-type hint after a function call on the RHS
            emit_return_type_hint(source, a.value, defs, range, out);
            hints_in_expr(source, a.target, defs, range, out);
            hints_in_expr(source, a.value, defs, range, out);
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
                out.push(make_param_hint(pos, param));
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
    if let Some(name) = name {
        if let Some(def) = defs.get(name) {
            if let Some(ret_type) = &def.return_type {
                if ret_type == "void" {
                    return;
                }
                let pos = offset_to_position(source, expr.span.end);
                if pos_in_range(pos, range) {
                    out.push(make_return_hint(pos, ret_type));
                }
            }
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

fn make_param_hint(position: Position, param_name: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!("{}:", param_name)),
        kind: Some(InlayHintKind::PARAMETER),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: Some(true),
        data: None,
    }
}

fn make_return_hint(position: Position, ret_type: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {ret_type}")),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: None,
        data: None,
    }
}

fn pos_in_range(pos: Position, range: Range) -> bool {
    pos.line >= range.start.line && pos.line <= range.end.line
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
}
