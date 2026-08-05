use std::collections::HashMap;

use php_ast::{
    ClassMemberKind, EnumMemberKind, Expr, ExprKind, NamespaceBody, Param, Stmt, StmtKind,
};
use serde_json::json;
use tower_lsp_server::ls_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range};

use crate::document::ast::{ParsedDoc, SourceView, format_type_hint};
use crate::text::fqn_short_name;

/// Resolve a foreach value/key variable's class (short name) for its type hint.
///
/// mir-primary: queries the recorded `ResolvedSymbol` at the variable's byte
/// offset — works whenever mir knows the array element type (e.g. typed arrays,
/// `@var list<T>` annotations, `enum::cases()`, and `array_map`/`array_filter`
/// results, whose element type mir infers from the callback return type).
fn foreach_var_class(
    analysis: Option<&mir_analyzer::FileAnalysis>,
    var_offset: u32,
) -> Option<String> {
    analysis
        .and_then(|a| crate::types::type_query::type_at_offset(a, var_offset))
        .and_then(crate::types::type_query::primary_class_name)
        .map(|fqcn| fqn_short_name(&fqcn).to_string())
}

/// Returns parameter-name inlay hints AND return-type hints for all
/// function/method declarations and calls in `doc`.
///
pub fn inlay_hints(
    _source: &str,
    doc: &ParsedDoc,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    session: Option<&mir_analyzer::AnalysisSession>,
    range: Range,
) -> Vec<InlayHint> {
    let sv = doc.view();
    let defs = collect_defs(&doc.program().stmts);
    let mut hints = Vec::new();
    let ctx = HintCtx {
        sv,
        defs: &defs,
        analysis,
        session,
        range,
    };
    hints_in_stmts(&ctx, &doc.program().stmts, &mut hints);
    hints
}

#[derive(Clone)]
struct CallableSignature {
    params: Vec<String>,
    variadic_last: bool,
    return_type: Option<String>,
    tooltip: Option<TooltipSymbol>,
}

#[derive(Clone)]
struct TooltipSymbol {
    kind: &'static str,
    name: String,
    class: Option<String>,
}

struct HintCtx<'a> {
    sv: SourceView<'a>,
    defs: &'a HashMap<String, CallableSignature>,
    analysis: Option<&'a mir_analyzer::FileAnalysis>,
    session: Option<&'a mir_analyzer::AnalysisSession>,
    range: Range,
}

// === Definition collection ===

fn collect_defs(stmts: &[Stmt<'_, '_>]) -> HashMap<String, CallableSignature> {
    let mut map = HashMap::new();
    collect_defs_stmts(stmts, &mut map);
    map
}

/// Extract param names and whether the last param is variadic from a param list.
fn params_from_list(params: &[Param<'_, '_>]) -> (Vec<String>, bool) {
    let names = params
        .iter()
        .map(|p| p.name.to_string().trim_start_matches('$').to_string())
        .collect();
    let variadic_last = params.last().map(|p| p.variadic).unwrap_or(false);
    (names, variadic_last)
}

fn collect_defs_stmts(stmts: &[Stmt<'_, '_>], map: &mut HashMap<String, CallableSignature>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let (params, variadic_last) = params_from_list(&f.params);
                let return_type = f.return_type.as_ref().map(|t| format_type_hint(t));
                map.insert(
                    f.name.to_string(),
                    CallableSignature {
                        params,
                        variadic_last,
                        return_type,
                        tooltip: Some(TooltipSymbol {
                            kind: "function",
                            name: f.name.to_string(),
                            class: None,
                        }),
                    },
                );
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let (params, variadic_last) = params_from_list(&m.params);
                        let return_type = m.return_type.as_ref().map(|t| format_type_hint(t));
                        let func_def = CallableSignature {
                            params: params.clone(),
                            variadic_last,
                            return_type: return_type.clone(),
                            tooltip: c.name.map(|cn| TooltipSymbol {
                                kind: "method",
                                name: m.name.to_string(),
                                class: Some(cn.to_string()),
                            }),
                        };
                        // Register with qualified key "ClassName::methodName" for unambiguous lookup
                        if let Some(cn) = c.name {
                            let qualified = format!("{}::{}", cn, m.name);
                            map.insert(qualified, func_def.clone());
                        }
                        // Register __construct under the class name so `new ClassName(...)` gets hints.
                        if m.name == "__construct"
                            && let Some(class_name) = c.name
                        {
                            map.insert(
                                class_name.to_string(),
                                CallableSignature {
                                    params: params.clone(),
                                    variadic_last,
                                    return_type: None,
                                    tooltip: Some(TooltipSymbol {
                                        kind: "class",
                                        name: class_name.to_string(),
                                        class: None,
                                    }),
                                },
                            );
                        }
                        map.insert(m.name.to_string(), func_def);
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let (params, variadic_last) = params_from_list(&m.params);
                        let return_type = m.return_type.as_ref().map(|t| format_type_hint(t));
                        let func_def = CallableSignature {
                            params,
                            variadic_last,
                            return_type,
                            tooltip: Some(TooltipSymbol {
                                kind: "method",
                                name: m.name.to_string(),
                                class: Some(t.name.to_string()),
                            }),
                        };
                        // Register with qualified key for unambiguous lookup
                        let qualified = format!("{}::{}", t.name, m.name);
                        map.insert(qualified, func_def.clone());
                        map.insert(m.name.to_string(), func_def);
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        let (params, variadic_last) = params_from_list(&m.params);
                        let return_type = m.return_type.as_ref().map(|t| format_type_hint(t));
                        let func_def = CallableSignature {
                            params,
                            variadic_last,
                            return_type,
                            tooltip: Some(TooltipSymbol {
                                kind: "method",
                                name: m.name.to_string(),
                                class: Some(e.name.to_string()),
                            }),
                        };
                        // Register with qualified key for unambiguous lookup
                        let qualified = format!("{}::{}", e.name, m.name);
                        map.insert(qualified, func_def.clone());
                        map.insert(m.name.to_string(), func_def);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_defs_stmts(&inner.stmts, map);
                }
            }
            // Register closure/arrow-function variables so `$fn(...)` call sites get hints.
            StmtKind::Expression(e) => {
                if let ExprKind::Assign(assign) = &e.kind
                    && let ExprKind::Variable(var_name) = &assign.target.kind
                {
                    let key = format!("${}", var_name.as_str());
                    match &assign.value.kind {
                        ExprKind::Closure(c) => {
                            let (params, variadic_last) = params_from_list(&c.params);
                            let return_type = c.return_type.as_ref().map(|t| format_type_hint(t));
                            map.insert(
                                key,
                                CallableSignature {
                                    params,
                                    variadic_last,
                                    return_type,
                                    tooltip: None,
                                },
                            );
                        }
                        ExprKind::ArrowFunction(a) => {
                            let (params, variadic_last) = params_from_list(&a.params);
                            let return_type = a.return_type.as_ref().map(|t| format_type_hint(t));
                            map.insert(
                                key,
                                CallableSignature {
                                    params,
                                    variadic_last,
                                    return_type,
                                    tooltip: None,
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

fn hints_in_stmts(ctx: &HintCtx<'_>, stmts: &[Stmt<'_, '_>], out: &mut Vec<InlayHint>) {
    for stmt in stmts {
        hints_in_stmt(ctx, stmt, out);
    }
}

fn hints_in_stmt(ctx: &HintCtx<'_>, stmt: &Stmt<'_, '_>, out: &mut Vec<InlayHint>) {
    match &stmt.kind {
        StmtKind::Expression(e) => hints_in_expr(ctx, e, out),
        StmtKind::Return(Some(v)) => hints_in_expr(ctx, v, out),
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                hints_in_expr(ctx, expr, out);
            }
        }
        StmtKind::Function(f) => hints_in_stmts(ctx, &f.body.stmts, out),
        StmtKind::Class(c) => {
            for member in c.body.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(ctx, &body.stmts, out);
                }
            }
        }
        StmtKind::Trait(t) => {
            for member in t.body.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(ctx, &body.stmts, out);
                }
            }
        }
        StmtKind::Enum(e) => {
            for member in e.body.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(ctx, &body.stmts, out);
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                hints_in_stmts(ctx, &inner.stmts, out);
            }
        }
        StmtKind::If(i) => {
            hints_in_expr(ctx, &i.condition, out);
            hints_in_stmt(ctx, i.then_branch, out);
            for ei in i.elseif_branches.iter() {
                hints_in_expr(ctx, &ei.condition, out);
                hints_in_stmt(ctx, &ei.body, out);
            }
            if let Some(e) = &i.else_branch {
                hints_in_stmt(ctx, e, out);
            }
        }
        StmtKind::While(w) => {
            hints_in_expr(ctx, &w.condition, out);
            hints_in_stmt(ctx, w.body, out);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                hints_in_expr(ctx, e, out);
            }
            for cond in f.condition.iter() {
                hints_in_expr(ctx, cond, out);
            }
            for e in f.update.iter() {
                hints_in_expr(ctx, e, out);
            }
            hints_in_stmt(ctx, f.body, out);
        }
        StmtKind::Foreach(f) => {
            hints_in_expr(ctx, &f.expr, out);
            // Emit type hint after the value variable, e.g. `foreach ($arr as $item /* : Foo */)`.
            if let ExprKind::Variable(_) = &f.value.kind
                && let Some(ty) = foreach_var_class(ctx.analysis, f.value.span.start)
            {
                let pos = ctx.sv.position_of(f.value.span.end);
                if pos_in_range(pos, ctx.range) {
                    out.push(make_foreach_type_hint(pos, &ty));
                }
            }
            // Emit type hint after the key variable if present, e.g. `foreach ($map as $key => $value)`.
            if let Some(key_expr) = &f.key
                && let ExprKind::Variable(_) = &key_expr.kind
                && let Some(ty) = foreach_var_class(ctx.analysis, key_expr.span.start)
            {
                let pos = ctx.sv.position_of(key_expr.span.end);
                if pos_in_range(pos, ctx.range) {
                    out.push(make_foreach_type_hint(pos, &ty));
                }
            }
            hints_in_stmt(ctx, f.body, out);
        }
        StmtKind::TryCatch(t) => {
            hints_in_stmts(ctx, &t.body.stmts, out);
            for catch in t.catches.iter() {
                hints_in_stmts(ctx, &catch.body.stmts, out);
            }
            if let Some(finally) = &t.finally {
                hints_in_stmts(ctx, &finally.stmts, out);
            }
        }
        StmtKind::Block(stmts) => hints_in_stmts(ctx, &stmts.stmts, out),
        StmtKind::DoWhile(d) => {
            hints_in_stmt(ctx, d.body, out);
            hints_in_expr(ctx, &d.condition, out);
        }
        StmtKind::Switch(s) => {
            hints_in_expr(ctx, &s.expr, out);
            for case in s.body.cases.iter() {
                if let Some(v) = &case.value {
                    hints_in_expr(ctx, v, out);
                }
                hints_in_stmts(ctx, &case.body, out);
            }
        }
        _ => {}
    }
}

fn hints_in_expr(ctx: &HintCtx<'_>, expr: &Expr<'_, '_>, out: &mut Vec<InlayHint>) {
    match &expr.kind {
        ExprKind::FunctionCall(f) => {
            let def = ident_name(f.name)
                .and_then(|_| callable_from_symbol(ctx, f.name.span.start))
                .or_else(|| callable_from_local_function(ctx, f.name));
            if let Some(def) = def {
                emit_param_hints(ctx, &f.args, &def, out);
            }
            hints_in_expr(ctx, f.name, out);
            for arg in f.args.iter() {
                if let Some(value) = &arg.value {
                    hints_in_expr(ctx, value, out);
                }
            }
        }
        ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
            if let Some(def) = callable_from_symbol(ctx, m.method.span.start) {
                emit_param_hints(ctx, &m.args, &def, out);
            }
            hints_in_expr(ctx, m.object, out);
            for arg in m.args.iter() {
                if let Some(value) = &arg.value {
                    hints_in_expr(ctx, value, out);
                }
            }
        }
        ExprKind::StaticMethodCall(m) => {
            if let Some(def) = callable_from_symbol(ctx, m.method.span.start) {
                emit_param_hints(ctx, &m.args, &def, out);
            }
            hints_in_expr(ctx, m.class, out);
            for arg in m.args.iter() {
                if let Some(value) = &arg.value {
                    hints_in_expr(ctx, value, out);
                }
            }
        }
        ExprKind::New(n) => {
            let def = callable_from_symbol(ctx, n.class.span.start).or_else(|| {
                ident_name(n.class).and_then(|class_name| ctx.defs.get(class_name).cloned())
            });
            if let Some(def) = def {
                emit_param_hints(ctx, &n.args, &def, out);
            }
            for arg in n.args.iter() {
                if let Some(value) = &arg.value {
                    hints_in_expr(ctx, value, out);
                }
            }
        }
        ExprKind::Assign(a) => {
            // Emit return-type hint after a function call on the RHS
            emit_return_type_hint(ctx, a.value, out);
            hints_in_expr(ctx, a.target, out);
            hints_in_expr(ctx, a.value, out);
        }
        // Walk into closure bodies so nested function calls get hints.
        ExprKind::Closure(c) => hints_in_stmts(ctx, &c.body.stmts, out),
        // Walk into arrow function bodies so nested calls get hints.
        // No return-type hint: the annotation is already visible in the source,
        // and php-lsp has no type inference to supply hints for unannotated fns.
        ExprKind::ArrowFunction(a) => hints_in_expr(ctx, a.body, out),
        ExprKind::Parenthesized(e) => hints_in_expr(ctx, e, out),
        ExprKind::Ternary(t) => {
            hints_in_expr(ctx, t.condition, out);
            if let Some(then_expr) = t.then_expr {
                hints_in_expr(ctx, then_expr, out);
            }
            hints_in_expr(ctx, t.else_expr, out);
        }
        ExprKind::NullCoalesce(n) => {
            hints_in_expr(ctx, n.left, out);
            hints_in_expr(ctx, n.right, out);
        }
        ExprKind::Binary(b) => {
            hints_in_expr(ctx, b.left, out);
            hints_in_expr(ctx, b.right, out);
        }
        ExprKind::CloneWith(target, withs) => {
            hints_in_expr(ctx, target, out);
            hints_in_expr(ctx, withs, out);
        }
        ExprKind::Match(m) => {
            hints_in_expr(ctx, m.subject, out);
            for arm in m.arms.iter() {
                if let Some(conds) = &arm.conditions {
                    for c in conds.iter() {
                        hints_in_expr(ctx, c, out);
                    }
                }
                hints_in_expr(ctx, &arm.body, out);
            }
        }
        _ => {}
    }
}

fn emit_param_hints(
    ctx: &HintCtx<'_>,
    args: &[php_ast::Arg<'_, '_>],
    def: &CallableSignature,
    out: &mut Vec<InlayHint>,
) {
    for (i, arg) in args.iter().enumerate() {
        // An unpacked/spread argument (`...$args`) consumes an unknown number
        // of positional parameters, so raw arg-list index no longer lines up
        // with the real parameter index for it *or anything after it* —
        // suppress hints for the rest of the call, not just the unpack itself.
        if arg.unpack {
            break;
        }
        // Skip named arguments (they already have the label in sv.source()).
        if arg.name.is_some() {
            continue;
        }
        // For a variadic last param, repeat its name for every excess argument.
        let param = if let Some(p) = def.params.get(i) {
            p
        } else if def.variadic_last {
            match def.params.last() {
                Some(p) => p,
                None => continue,
            }
        } else {
            continue;
        };
        let pos = ctx.sv.position_of(arg.span.start);
        if pos_in_range(pos, ctx.range) {
            out.push(make_param_hint(pos, param, def.tooltip.as_ref()));
        }
    }
}

fn emit_return_type_hint(ctx: &HintCtx<'_>, expr: &Expr<'_, '_>, out: &mut Vec<InlayHint>) {
    let def = match &expr.kind {
        ExprKind::FunctionCall(f) => {
            ident_name(f.name).and_then(|_| callable_from_symbol(ctx, f.name.span.start))
        }
        ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
            callable_from_symbol(ctx, m.method.span.start)
        }
        ExprKind::StaticMethodCall(m) => callable_from_symbol(ctx, m.method.span.start),
        _ => None,
    };
    if let Some(def) = def
        && let Some(ret_type) = &def.return_type
    {
        if ret_type == "void" {
            return;
        }
        let pos = ctx.sv.position_of(expr.span.end);
        if pos_in_range(pos, ctx.range) {
            out.push(make_return_hint(pos, ret_type, def.tooltip.as_ref()));
        }
    }
}

fn callable_from_local_function(
    ctx: &HintCtx<'_>,
    expr: &Expr<'_, '_>,
) -> Option<CallableSignature> {
    ident_name(expr)
        .map(str::to_string)
        .or_else(|| {
            if let ExprKind::Variable(n) = &expr.kind {
                Some(format!("${}", n.as_str()))
            } else {
                None
            }
        })
        .and_then(|key| ctx.defs.get(&key).cloned())
}

fn callable_from_symbol(ctx: &HintCtx<'_>, offset: u32) -> Option<CallableSignature> {
    let symbol = ctx.analysis?.symbol_at(offset)?.to_symbol()?;
    callable_signature_from_name(ctx.session?, &symbol)
}

fn callable_signature_from_name(
    session: &mir_analyzer::AnalysisSession,
    symbol: &mir_analyzer::Name,
) -> Option<CallableSignature> {
    let db = session.snapshot_db();
    match symbol {
        mir_analyzer::Name::Function(fqn) => {
            let f = mir_analyzer::db::find_function(
                &db,
                mir_analyzer::db::Fqcn::from_str(&db, fqn.as_ref()),
            )?;
            if mir_analyzer::is_builtin_function(&f.short_name) {
                return None;
            }
            Some(CallableSignature {
                params: declared_param_names(&f.params),
                variadic_last: f.params.last().is_some_and(|p| p.is_variadic),
                return_type: f.effective_return_type().map(ToString::to_string),
                tooltip: Some(TooltipSymbol {
                    kind: "function",
                    name: f.short_name.to_string(),
                    class: None,
                }),
            })
        }
        mir_analyzer::Name::Method { class, name } => {
            let (_, m) = mir_analyzer::db::find_method_in_chain(
                &db,
                mir_analyzer::db::Fqcn::from_str(&db, class.as_ref()),
                name.as_ref(),
            )?;
            Some(CallableSignature {
                params: declared_param_names(&m.params),
                variadic_last: m.params.last().is_some_and(|p| p.is_variadic),
                return_type: m
                    .return_type
                    .as_deref()
                    .or(m.inferred_return_type.as_deref())
                    .map(ToString::to_string),
                tooltip: Some(TooltipSymbol {
                    kind: "method",
                    name: m.name.to_string(),
                    class: Some(class.to_string()),
                }),
            })
        }
        mir_analyzer::Name::Class(fqcn) => {
            let (_, m) = mir_analyzer::db::find_method_in_chain(
                &db,
                mir_analyzer::db::Fqcn::from_str(&db, fqcn.as_ref()),
                "__construct",
            )?;
            Some(CallableSignature {
                params: declared_param_names(&m.params),
                variadic_last: m.params.last().is_some_and(|p| p.is_variadic),
                return_type: None,
                tooltip: Some(TooltipSymbol {
                    kind: "class",
                    name: fqcn.to_string(),
                    class: None,
                }),
            })
        }
        _ => None,
    }
}

fn declared_param_names(params: &[mir_analyzer::DeclaredParam]) -> Vec<String> {
    params
        .iter()
        .map(|p| p.name.as_str().trim_start_matches('$').to_string())
        .collect()
}

fn ident_name<'a>(expr: &'a Expr<'_, '_>) -> Option<&'a str> {
    if let ExprKind::Identifier(name) = &expr.kind {
        Some(name)
    } else {
        None
    }
}

fn make_param_hint(
    position: Position,
    param_name: &str,
    symbol: Option<&TooltipSymbol>,
) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!("{}:", param_name)),
        kind: Some(InlayHintKind::PARAMETER),
        text_edits: None,
        tooltip: None,
        padding_left: None,
        padding_right: Some(true),
        data: hint_data(symbol),
    }
}

fn make_return_hint(
    position: Position,
    ret_type: &str,
    symbol: Option<&TooltipSymbol>,
) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {ret_type}")),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: None,
        data: hint_data(symbol),
    }
}

fn hint_data(symbol: Option<&TooltipSymbol>) -> Option<serde_json::Value> {
    let symbol = symbol?;
    let mut value = json!({
        "php_lsp_fn": symbol.name,
        "php_lsp_symbol_kind": symbol.kind,
    });
    if let Some(class) = &symbol.class {
        value["php_lsp_class"] = json!(class);
    }
    Some(value)
}

fn make_foreach_type_hint(position: Position, ty: &str) -> InlayHint {
    InlayHint {
        position,
        label: InlayHintLabel::String(format!(": {ty}")),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: None,
        data: None,
    }
}

fn pos_in_range(pos: Position, range: Range) -> bool {
    if pos.line < range.start.line || pos.line > range.end.line {
        return false;
    }
    if pos.line == range.start.line && pos.character < range.start.character {
        return false;
    }
    if pos.line == range.end.line && pos.character >= range.end.character {
        return false;
    }
    true
}
