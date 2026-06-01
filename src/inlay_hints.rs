use std::collections::HashMap;
use std::sync::Arc;

use php_ast::{
    ClassMemberKind, EnumMemberKind, Expr, ExprKind, NamespaceBody, Param, Stmt, StmtKind,
};
use serde_json::json;
use tower_lsp::lsp_types::{InlayHint, InlayHintKind, InlayHintLabel, Position, Range, Url};

use crate::ast::{MethodReturnsMap, ParsedDoc, SourceView, format_type_hint};
use crate::file_index::FileIndex;
use crate::type_map::TypeMap;
use crate::util::fqn_short_name;

/// Resolve a foreach value/key variable's class (short name) for its type hint.
/// mir-primary at the variable's byte offset; TypeMap fallback for binding
/// sites mir resolves to `mixed` (e.g. element type it can't infer).
fn foreach_var_class(
    type_map: &TypeMap,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    var_key: &str,
    var_offset: u32,
) -> Option<String> {
    analysis
        .and_then(|a| crate::type_query::type_at_offset(a, var_offset))
        .and_then(crate::type_query::primary_class_name)
        .map(|fqcn| fqn_short_name(&fqcn).to_string())
        .or_else(|| type_map.get(var_key).map(str::to_owned))
}

#[derive(Clone)]
struct FuncDef {
    params: Vec<String>,
    /// Whether the last parameter is variadic (`...$name`).
    variadic_last: bool,
    return_type: Option<String>,
}

/// Returns parameter-name inlay hints AND return-type hints for all
/// function/method declarations and calls in `doc`.
///
/// `workspace_files` is the list of all indexed files; definitions not found
/// in the current document fall back to this workspace index so that calls to
/// cross-file functions/methods still get parameter-name hints.
pub fn inlay_hints(
    _source: &str,
    doc: &ParsedDoc,
    doc_returns: Option<&MethodReturnsMap>,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    range: Range,
    workspace_files: &[(Url, Arc<FileIndex>)],
) -> Vec<InlayHint> {
    let sv = doc.view();
    let mut defs = collect_defs(&doc.program().stmts);
    collect_defs_from_workspace(workspace_files, &mut defs);
    let type_map = TypeMap::from_doc_with_meta(doc, None, doc_returns);
    let mut hints = Vec::new();
    hints_in_stmts(
        sv,
        &doc.program().stmts,
        &defs,
        &type_map,
        analysis,
        range,
        &mut hints,
    );
    hints
}

// === Definition collection ===

fn collect_defs(stmts: &[Stmt<'_, '_>]) -> HashMap<String, FuncDef> {
    let mut map = HashMap::new();
    collect_defs_stmts(stmts, &mut map);
    map
}

/// Populate `map` with function and method definitions from the workspace index.
/// Entries already present (from the current file's AST) are not overwritten so
/// that the in-file definition always wins over a potentially stale index entry.
fn collect_defs_from_workspace(
    workspace_files: &[(Url, Arc<FileIndex>)],
    map: &mut HashMap<String, FuncDef>,
) {
    for (_, idx) in workspace_files {
        for func in &idx.functions {
            let func_name = func.name.to_string();
            if map.contains_key(&func_name) {
                continue;
            }
            let params: Vec<String> = func.params.iter().map(|p| p.name.to_string()).collect();
            let variadic_last = func.params.last().map(|p| p.variadic).unwrap_or(false);
            map.insert(
                func_name,
                FuncDef {
                    params,
                    variadic_last,
                    return_type: func.return_type.as_ref().map(|r| r.to_string()),
                },
            );
        }
        for class in &idx.classes {
            for method in &class.methods {
                let method_name = method.name.to_string();
                let params: Vec<String> =
                    method.params.iter().map(|p| p.name.to_string()).collect();
                let variadic_last = method.params.last().map(|p| p.variadic).unwrap_or(false);
                let func_def = FuncDef {
                    params: params.clone(),
                    variadic_last,
                    return_type: method.return_type.as_ref().map(|r| r.to_string()),
                };
                // Register with qualified key "ClassName::methodName" for unambiguous lookup
                let cn = class.name.as_ref();
                let qualified = format!("{}::{}", cn, method_name);
                map.insert(qualified, func_def.clone());
                // Also register __construct under the class name so `new ClassName(...)` gets hints.
                if method_name == "__construct" {
                    map.entry(cn.to_string()).or_insert_with(|| FuncDef {
                        params: params.clone(),
                        variadic_last,
                        return_type: None,
                    });
                }
                // Register with short name as fallback for backwards compatibility
                map.entry(method_name).or_insert(func_def);
            }
        }
    }
}

/// Extract param names and whether the last param is variadic from a param list.
fn params_from_list(params: &[Param<'_, '_>]) -> (Vec<String>, bool) {
    let names = params.iter().map(|p| p.name.to_string()).collect();
    let variadic_last = params.last().map(|p| p.variadic).unwrap_or(false);
    (names, variadic_last)
}

fn collect_defs_stmts(stmts: &[Stmt<'_, '_>], map: &mut HashMap<String, FuncDef>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let (params, variadic_last) = params_from_list(&f.params);
                let return_type = f.return_type.as_ref().map(|t| format_type_hint(t));
                map.insert(
                    f.name.to_string(),
                    FuncDef {
                        params,
                        variadic_last,
                        return_type,
                    },
                );
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let (params, variadic_last) = params_from_list(&m.params);
                        let return_type = m.return_type.as_ref().map(|t| format_type_hint(t));
                        let func_def = FuncDef {
                            params: params.clone(),
                            variadic_last,
                            return_type: return_type.clone(),
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
                                FuncDef {
                                    params: params.clone(),
                                    variadic_last,
                                    return_type: None,
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
                        let func_def = FuncDef {
                            params,
                            variadic_last,
                            return_type,
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
                        let func_def = FuncDef {
                            params,
                            variadic_last,
                            return_type,
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
                                FuncDef {
                                    params,
                                    variadic_last,
                                    return_type,
                                },
                            );
                        }
                        ExprKind::ArrowFunction(a) => {
                            let (params, variadic_last) = params_from_list(&a.params);
                            let return_type = a.return_type.as_ref().map(|t| format_type_hint(t));
                            map.insert(
                                key,
                                FuncDef {
                                    params,
                                    variadic_last,
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
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    defs: &HashMap<String, FuncDef>,
    type_map: &TypeMap,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    for stmt in stmts {
        hints_in_stmt(sv, stmt, defs, type_map, analysis, range, out);
    }
}

fn hints_in_stmt(
    sv: SourceView<'_>,
    stmt: &Stmt<'_, '_>,
    defs: &HashMap<String, FuncDef>,
    type_map: &TypeMap,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    match &stmt.kind {
        StmtKind::Expression(e) => hints_in_expr(sv, e, defs, type_map, analysis, range, out),
        StmtKind::Return(Some(v)) => hints_in_expr(sv, v, defs, type_map, analysis, range, out),
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                hints_in_expr(sv, expr, defs, type_map, analysis, range, out);
            }
        }
        StmtKind::Function(f) => {
            hints_in_stmts(sv, &f.body.stmts, defs, type_map, analysis, range, out);
        }
        StmtKind::Class(c) => {
            for member in c.body.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(sv, &body.stmts, defs, type_map, analysis, range, out);
                }
            }
        }
        StmtKind::Trait(t) => {
            for member in t.body.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(sv, &body.stmts, defs, type_map, analysis, range, out);
                }
            }
        }
        StmtKind::Enum(e) => {
            for member in e.body.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    hints_in_stmts(sv, &body.stmts, defs, type_map, analysis, range, out);
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                hints_in_stmts(sv, &inner.stmts, defs, type_map, analysis, range, out);
            }
        }
        StmtKind::If(i) => {
            hints_in_expr(sv, &i.condition, defs, type_map, analysis, range, out);
            hints_in_stmt(sv, i.then_branch, defs, type_map, analysis, range, out);
            for ei in i.elseif_branches.iter() {
                hints_in_expr(sv, &ei.condition, defs, type_map, analysis, range, out);
                hints_in_stmt(sv, &ei.body, defs, type_map, analysis, range, out);
            }
            if let Some(e) = &i.else_branch {
                hints_in_stmt(sv, e, defs, type_map, analysis, range, out);
            }
        }
        StmtKind::While(w) => {
            hints_in_expr(sv, &w.condition, defs, type_map, analysis, range, out);
            hints_in_stmt(sv, w.body, defs, type_map, analysis, range, out);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                hints_in_expr(sv, e, defs, type_map, analysis, range, out);
            }
            for cond in f.condition.iter() {
                hints_in_expr(sv, cond, defs, type_map, analysis, range, out);
            }
            for e in f.update.iter() {
                hints_in_expr(sv, e, defs, type_map, analysis, range, out);
            }
            hints_in_stmt(sv, f.body, defs, type_map, analysis, range, out);
        }
        StmtKind::Foreach(f) => {
            hints_in_expr(sv, &f.expr, defs, type_map, analysis, range, out);
            // Emit type hint after the value variable, e.g. `foreach ($arr as $item /* : Foo */)`.
            if let ExprKind::Variable(val_name) = &f.value.kind {
                let key = format!("${}", val_name.as_str());
                if let Some(ty) = foreach_var_class(type_map, analysis, &key, f.value.span.start) {
                    let pos = sv.position_of(f.value.span.end);
                    if pos_in_range(pos, range) {
                        out.push(make_foreach_type_hint(pos, &ty));
                    }
                }
            }
            // Emit type hint after the key variable if present, e.g. `foreach ($map as $key => $value)`.
            if let Some(key_expr) = &f.key
                && let ExprKind::Variable(key_name) = &key_expr.kind
            {
                let key = format!("${}", key_name.as_str());
                if let Some(ty) = foreach_var_class(type_map, analysis, &key, key_expr.span.start) {
                    let pos = sv.position_of(key_expr.span.end);
                    if pos_in_range(pos, range) {
                        out.push(make_foreach_type_hint(pos, &ty));
                    }
                }
            }
            hints_in_stmt(sv, f.body, defs, type_map, analysis, range, out);
        }
        StmtKind::TryCatch(t) => {
            hints_in_stmts(sv, &t.body.stmts, defs, type_map, analysis, range, out);
            for catch in t.catches.iter() {
                hints_in_stmts(sv, &catch.body.stmts, defs, type_map, analysis, range, out);
            }
            if let Some(finally) = &t.finally {
                hints_in_stmts(sv, &finally.stmts, defs, type_map, analysis, range, out);
            }
        }
        StmtKind::Block(stmts) => {
            hints_in_stmts(sv, &stmts.stmts, defs, type_map, analysis, range, out)
        }
        _ => {}
    }
}

fn hints_in_expr(
    sv: SourceView<'_>,
    expr: &Expr<'_, '_>,
    defs: &HashMap<String, FuncDef>,
    type_map: &TypeMap,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    match &expr.kind {
        ExprKind::FunctionCall(f) => {
            // Look up by identifier name or by variable name (for closure vars like `$fn(...)`).
            let key: Option<String> = ident_name(f.name).map(|n| n.to_string()).or_else(|| {
                if let ExprKind::Variable(n) = &f.name.kind {
                    Some(format!("${}", n.as_str()))
                } else {
                    None
                }
            });
            if let Some(k) = key
                && let Some(def) = defs.get(&k)
            {
                emit_param_hints(sv, &f.args, def, &k, range, out);
            }
            hints_in_expr(sv, f.name, defs, type_map, analysis, range, out);
            for arg in f.args.iter() {
                hints_in_expr(sv, &arg.value, defs, type_map, analysis, range, out);
            }
        }
        ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
            if let Some(name) = ident_name(m.method)
                && let Some(def) = defs.get(name)
            {
                emit_param_hints(sv, &m.args, def, name, range, out);
            }
            hints_in_expr(sv, m.object, defs, type_map, analysis, range, out);
            for arg in m.args.iter() {
                hints_in_expr(sv, &arg.value, defs, type_map, analysis, range, out);
            }
        }
        ExprKind::StaticMethodCall(m) => {
            if let Some(name) = ident_name(m.method)
                && let Some(def) = defs.get(name)
            {
                emit_param_hints(sv, &m.args, def, name, range, out);
            }
            hints_in_expr(sv, m.class, defs, type_map, analysis, range, out);
            for arg in m.args.iter() {
                hints_in_expr(sv, &arg.value, defs, type_map, analysis, range, out);
            }
        }
        ExprKind::New(n) => {
            if let Some(class_name) = ident_name(n.class)
                && let Some(def) = defs.get(class_name)
            {
                emit_param_hints(sv, &n.args, def, class_name, range, out);
            }
            for arg in n.args.iter() {
                hints_in_expr(sv, &arg.value, defs, type_map, analysis, range, out);
            }
        }
        ExprKind::Assign(a) => {
            // Emit return-type hint after a function call on the RHS
            emit_return_type_hint(sv, a.value, defs, range, out);
            hints_in_expr(sv, a.target, defs, type_map, analysis, range, out);
            hints_in_expr(sv, a.value, defs, type_map, analysis, range, out);
        }
        // Walk into closure bodies so nested function calls get hints.
        ExprKind::Closure(c) => {
            hints_in_stmts(sv, &c.body.stmts, defs, type_map, analysis, range, out);
        }
        // Walk into arrow function bodies so nested calls get hints.
        // No return-type hint: the annotation is already visible in the source,
        // and php-lsp has no type inference to supply hints for unannotated fns.
        ExprKind::ArrowFunction(a) => {
            hints_in_expr(sv, a.body, defs, type_map, analysis, range, out);
        }
        ExprKind::Parenthesized(e) => hints_in_expr(sv, e, defs, type_map, analysis, range, out),
        ExprKind::Ternary(t) => {
            hints_in_expr(sv, t.condition, defs, type_map, analysis, range, out);
            if let Some(then_expr) = t.then_expr {
                hints_in_expr(sv, then_expr, defs, type_map, analysis, range, out);
            }
            hints_in_expr(sv, t.else_expr, defs, type_map, analysis, range, out);
        }
        ExprKind::NullCoalesce(n) => {
            hints_in_expr(sv, n.left, defs, type_map, analysis, range, out);
            hints_in_expr(sv, n.right, defs, type_map, analysis, range, out);
        }
        ExprKind::Binary(b) => {
            hints_in_expr(sv, b.left, defs, type_map, analysis, range, out);
            hints_in_expr(sv, b.right, defs, type_map, analysis, range, out);
        }
        ExprKind::CloneWith(target, withs) => {
            hints_in_expr(sv, target, defs, type_map, analysis, range, out);
            hints_in_expr(sv, withs, defs, type_map, analysis, range, out);
        }
        _ => {}
    }
}

fn emit_param_hints(
    sv: SourceView<'_>,
    args: &[php_ast::Arg<'_, '_>],
    def: &FuncDef,
    func_name: &str,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    for (i, arg) in args.iter().enumerate() {
        // Skip named arguments (they already have the label in sv.source())
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
        let pos = sv.position_of(arg.span.start);
        if pos_in_range(pos, range) {
            out.push(make_param_hint(pos, param, func_name));
        }
    }
}

fn emit_return_type_hint(
    sv: SourceView<'_>,
    expr: &Expr<'_, '_>,
    defs: &HashMap<String, FuncDef>,
    range: Range,
    out: &mut Vec<InlayHint>,
) {
    let name = match &expr.kind {
        ExprKind::FunctionCall(f) => ident_name(f.name),
        ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => ident_name(m.method),
        ExprKind::StaticMethodCall(m) => ident_name(m.method),
        _ => return,
    };
    if let Some(name) = name
        && let Some(def) = defs.get(name)
        && let Some(ret_type) = &def.return_type
    {
        if ret_type == "void" {
            return;
        }
        let pos = sv.position_of(expr.span.end);
        if pos_in_range(pos, range) {
            out.push(make_return_hint(pos, ret_type, name));
        }
    }
}

fn ident_name<'a>(expr: &'a Expr<'_, '_>) -> Option<&'a str> {
    if let ExprKind::Identifier(name) = &expr.kind {
        Some(name)
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
