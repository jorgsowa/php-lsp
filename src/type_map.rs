/// Single-pass type inference: collects `$var = new ClassName()` assignments
/// to map variable names to class names.  Used to scope method completions
/// after `->`.
use std::collections::HashMap;

use php_ast::{
    BinaryOp, ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind, TypeHint,
    TypeHintKind,
};
use tower_lsp::lsp_types::Position;

use crate::ast::{MethodReturnsMap, ParsedDoc, SourceView};
use crate::docblock::{docblock_before, parse_docblock};
use crate::phpstorm_meta::PhpStormMeta;

/// Maps variable name (with `$`) → class name.
#[derive(Debug, Default, Clone)]
pub struct TypeMap(HashMap<String, String>);

impl TypeMap {
    /// Build from a parsed document. Method-return-type inference rebuilds
    /// the per-doc map inline — prefer [`from_doc_with_meta`] with a salsa-
    /// memoized `doc_returns` on hot paths.
    #[cfg(test)]
    pub fn from_doc(doc: &ParsedDoc) -> Self {
        Self::from_doc_with_meta(doc, None, None)
    }

    /// Build from a parsed document, optionally enriched by PHPStorm metadata
    /// for factory-method return type inference. `doc_returns` is the
    /// precomputed method-return map (typically from the salsa `method_returns`
    /// query); pass `None` to build it inline.
    pub fn from_doc_with_meta(
        doc: &ParsedDoc,
        meta: Option<&PhpStormMeta>,
        doc_returns: Option<&MethodReturnsMap>,
    ) -> Self {
        let owned;
        let returns: &MethodReturnsMap = match doc_returns {
            Some(r) => r,
            None => {
                owned = build_method_returns(doc);
                &owned
            }
        };
        let mut map = HashMap::new();
        collect_types_stmts(
            doc.source(),
            &doc.program().stmts,
            &mut map,
            meta,
            std::slice::from_ref(&returns),
            None,
        );
        TypeMap(map)
    }

    /// Build from a parsed document plus cross-file docs. Callers must supply
    /// precomputed method-return maps for the primary doc and each other doc
    /// (typically from the salsa `method_returns` query).
    pub fn from_docs_with_meta<'a>(
        doc: &ParsedDoc,
        doc_returns: &MethodReturnsMap,
        other_docs: impl IntoIterator<Item = (&'a ParsedDoc, &'a MethodReturnsMap)>,
        meta: Option<&'a PhpStormMeta>,
    ) -> Self {
        let mut all_returns: Vec<&MethodReturnsMap> = vec![doc_returns];
        all_returns.extend(other_docs.into_iter().map(|(_, r)| r));
        let mut map = HashMap::new();
        collect_types_stmts(
            doc.source(),
            &doc.program().stmts,
            &mut map,
            meta,
            &all_returns,
            None,
        );
        TypeMap(map)
    }

    /// Like [`from_docs_with_meta`] but scopes method-body variable collection
    /// to the method (or function) containing `position`. Variables local to
    /// other method bodies are excluded so they cannot pollute the map with
    /// wrong types at the cursor site. Sets `$this` when the position is inside
    /// an instance method.
    pub fn from_docs_at_position<'a>(
        doc: &ParsedDoc,
        doc_returns: &MethodReturnsMap,
        other_docs: impl IntoIterator<Item = (&'a ParsedDoc, &'a MethodReturnsMap)>,
        meta: Option<&'a PhpStormMeta>,
        position: Position,
    ) -> Self {
        let cursor_byte = {
            let line_starts = doc.line_starts();
            let line = position.line as usize;
            if line < line_starts.len() {
                let line_start = line_starts[line] as usize;
                let col_byte = crate::util::utf16_offset_to_byte(
                    &doc.source()[line_start..],
                    position.character as usize,
                );
                Some((line_start + col_byte) as u32)
            } else {
                None
            }
        };
        let mut all_returns: Vec<&MethodReturnsMap> = vec![doc_returns];
        all_returns.extend(other_docs.into_iter().map(|(_, r)| r));
        let mut map = HashMap::new();
        collect_types_stmts(
            doc.source(),
            &doc.program().stmts,
            &mut map,
            meta,
            &all_returns,
            cursor_byte,
        );
        TypeMap(map)
    }

    /// Returns the class name for a variable, e.g. `get("$obj")` → `Some("Foo")`.
    pub fn get<'a>(&'a self, var: &str) -> Option<&'a str> {
        self.0.get(var).map(|s| s.as_str())
    }
}

/// Pre-build a map of class_name → method_name → return_class_name for a single doc.
pub fn build_method_returns(doc: &ParsedDoc) -> MethodReturnsMap {
    let mut out = HashMap::new();
    collect_method_returns_stmts(doc.source(), &doc.program().stmts, &mut out);
    out
}

/// Look up `class.method() -> return_class` across a stack of per-doc maps.
/// Returns the first match — later docs override earlier ones, matching the
/// previous merge-based behavior.
fn lookup_method_return<'a>(
    maps: &'a [&'a MethodReturnsMap],
    class_name: &str,
    method_name: &str,
) -> Option<&'a str> {
    for m in maps.iter().rev() {
        if let Some(class_rets) = m.get(class_name)
            && let Some(ret) = class_rets.get(method_name)
        {
            return Some(ret.as_str());
        }
    }
    None
}

fn collect_method_returns_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    out: &mut HashMap<String, HashMap<String, String>>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let class_name = match c.name {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && let Some(ret) =
                            extract_method_return_class(source, member.span.start, m, &class_name)
                    {
                        out.entry(class_name.clone())
                            .or_default()
                            .insert(m.name.to_string(), ret);
                    }
                }
            }
            StmtKind::Trait(t) => {
                let trait_name = t.name.to_string();
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && let Some(ret) =
                            extract_method_return_class(source, member.span.start, m, &trait_name)
                    {
                        out.entry(trait_name.clone())
                            .or_default()
                            .insert(m.name.to_string(), ret);
                    }
                }
            }
            StmtKind::Enum(e) => {
                let enum_name = e.name.to_string();
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && let Some(ret) =
                            extract_method_return_class(source, member.span.start, m, &enum_name)
                    {
                        out.entry(enum_name.clone())
                            .or_default()
                            .insert(m.name.to_string(), ret);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_method_returns_stmts(source, inner, out);
                }
            }
            _ => {}
        }
    }
}

fn extract_method_return_class(
    source: &str,
    member_start: u32,
    m: &php_ast::MethodDecl<'_, '_>,
    enclosing_class: &str,
) -> Option<String> {
    // 1. AST return type hint takes priority
    if let Some(hint) = &m.return_type
        && let Some(s) = type_hint_to_class_string(hint, Some(enclosing_class))
    {
        return Some(s);
    }
    // 2. @return docblock fallback
    if let Some(raw) = docblock_before(source, member_start) {
        let db = parse_docblock(&raw);
        if let Some(ret) = db.return_type {
            for part in ret.type_hint.split('|') {
                let part = part.trim().trim_start_matches('\\').trim_start_matches('?');
                let short = part.rsplit('\\').next().unwrap_or(part);
                if short == "self" || short == "static" {
                    return Some(enclosing_class.to_string());
                }
                let first = short.chars().next().unwrap_or('_');
                if first.is_uppercase() && !matches!(short, "void" | "never" | "null") {
                    return Some(short.to_string());
                }
            }
        }
    }
    None
}

/// Extract a class-name string from a type hint using mir's type resolver.
/// - `Named(Foo)` → `"Foo"`, `Named(\App\Foo)` → `"Foo"` (short name)
/// - `Nullable(Named(Foo))` → `"Foo"` (strips the nullable wrapper)
/// - `Union([Named(Foo), Named(Bar)])` → `"Foo|Bar"`
/// - `self` / `static` with `enclosing` → returns the enclosing short name
/// - Primitives and unrecognised kinds → `None`
fn type_hint_to_class_string(
    hint: &TypeHint<'_, '_>,
    enclosing_class: Option<&str>,
) -> Option<String> {
    use mir_types::Atomic;
    let union = mir_analyzer::parser::type_from_hint(hint, enclosing_class);
    let classes: Vec<String> = union
        .types
        .iter()
        .filter_map(|a| match a {
            Atomic::TNamedObject { fqcn, .. }
            | Atomic::TSelf { fqcn }
            | Atomic::TStaticObject { fqcn }
            | Atomic::TParent { fqcn } => {
                let short = fqcn.rsplit('\\').next().unwrap_or(fqcn.as_ref());
                Some(short.to_string())
            }
            _ => None,
        })
        .collect();
    if classes.is_empty() {
        None
    } else {
        Some(classes.join("|"))
    }
}

fn collect_types_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    map: &mut HashMap<String, String>,
    meta: Option<&PhpStormMeta>,
    method_returns: &[&MethodReturnsMap],
    cursor_byte: Option<u32>,
) {
    for stmt in stmts {
        // Check for `/** @var ClassName $varName */` docblock before this statement.
        if let Some(raw) = docblock_before(source, stmt.span.start) {
            let db = parse_docblock(&raw);
            if let Some(type_str) = db.var_type {
                // Only map object types (starts with uppercase or backslash).
                // type_str may be a union like "Foo|null"; take the first class part.
                let class_name = type_str
                    .split('|')
                    .map(|p| p.trim().trim_start_matches('\\').trim_start_matches('?'))
                    .find(|p| p.chars().next().map(|c| c.is_uppercase()).unwrap_or(false))
                    .and_then(|p| p.rsplit('\\').next())
                    .map(|p| p.to_string());
                if let Some(class_name) = class_name {
                    if let Some(vname) = db.var_name {
                        // `@var Foo $obj` — explicit variable name.
                        map.insert(format!("${}", vname.as_str()), class_name);
                    } else if let StmtKind::Expression(e) = &stmt.kind {
                        // `@var Foo` above `$obj = ...` — infer from the LHS.
                        if let ExprKind::Assign(a) = &e.kind
                            && let ExprKind::Variable(vn) = &a.target.kind
                        {
                            map.insert(format!("${}", vn.as_str()), class_name);
                        }
                    }
                }
            }
        }

        match &stmt.kind {
            StmtKind::Expression(e) => {
                collect_types_expr(source, e, map, meta, method_returns, cursor_byte)
            }
            StmtKind::Function(f) => {
                // Only collect params/body when cursor is inside this function (or no cursor).
                let in_scope =
                    cursor_byte.is_none_or(|c| stmt.span.start <= c && c <= stmt.span.end);
                if !in_scope {
                    continue;
                }
                // Read @param docblock hints — fills in types for untyped params
                if let Some(raw) = docblock_before(source, stmt.span.start) {
                    let db = parse_docblock(&raw);
                    for param in &db.params {
                        // For union types, collect all class parts joined by |
                        let classes: Vec<&str> = param
                            .type_hint
                            .split('|')
                            .map(|p| p.trim().trim_start_matches('\\').trim_start_matches('?'))
                            .filter(|p| p.chars().next().map(|c| c.is_uppercase()).unwrap_or(false))
                            .filter_map(|p| p.rsplit('\\').next())
                            .collect();
                        if !classes.is_empty() {
                            let key = if param.name.starts_with('$') {
                                param.name.clone()
                            } else {
                                format!("${}", param.name)
                            };
                            map.entry(key).or_insert_with(|| classes.join("|"));
                        }
                    }
                }
                for p in f.params.iter() {
                    if let Some(hint) = &p.type_hint
                        && let Some(class_str) = type_hint_to_class_string(hint, None)
                    {
                        map.insert(format!("${}", p.name), class_str);
                    }
                }
                collect_types_stmts(source, &f.body, map, meta, method_returns, cursor_byte);
            }
            StmtKind::Class(c) => {
                let class_name = c.name.map(|n| n.to_string());
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        // Only collect params/body when cursor is inside this method (or no cursor).
                        let in_scope = cursor_byte
                            .is_none_or(|cb| member.span.start <= cb && cb <= member.span.end);
                        if !in_scope {
                            continue;
                        }
                        // Read @param docblock hints — fills in types for untyped params
                        if let Some(raw) = docblock_before(source, member.span.start) {
                            let db = parse_docblock(&raw);
                            for param in &db.params {
                                // For union types, collect all class parts joined by |
                                let classes: Vec<&str> = param
                                    .type_hint
                                    .split('|')
                                    .map(|p| {
                                        p.trim().trim_start_matches('\\').trim_start_matches('?')
                                    })
                                    .filter(|p| {
                                        p.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                                    })
                                    .filter_map(|p| p.rsplit('\\').next())
                                    .collect();
                                if !classes.is_empty() {
                                    let key = if param.name.starts_with('$') {
                                        param.name.clone()
                                    } else {
                                        format!("${}", param.name)
                                    };
                                    map.entry(key).or_insert_with(|| classes.join("|"));
                                }
                            }
                        }
                        for p in m.params.iter() {
                            if let Some(hint) = &p.type_hint
                                && let Some(class_str) =
                                    type_hint_to_class_string(hint, class_name.as_deref())
                            {
                                map.insert(format!("${}", p.name), class_str);
                            }
                        }
                        // Set $this to the enclosing class for instance methods.
                        if !m.is_static
                            && let Some(ref cname) = class_name
                        {
                            map.insert("$this".to_string(), cname.clone());
                        }
                        if let Some(body) = &m.body {
                            collect_types_stmts(
                                source,
                                body,
                                map,
                                meta,
                                method_returns,
                                cursor_byte,
                            );
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let in_scope = cursor_byte
                            .is_none_or(|cb| member.span.start <= cb && cb <= member.span.end);
                        if !in_scope {
                            continue;
                        }
                        for p in m.params.iter() {
                            if let Some(hint) = &p.type_hint
                                && let Some(class_str) = type_hint_to_class_string(hint, None)
                            {
                                map.insert(format!("${}", p.name), class_str);
                            }
                        }
                        if let Some(body) = &m.body {
                            collect_types_stmts(
                                source,
                                body,
                                map,
                                meta,
                                method_returns,
                                cursor_byte,
                            );
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        let in_scope = cursor_byte
                            .is_none_or(|cb| member.span.start <= cb && cb <= member.span.end);
                        if !in_scope {
                            continue;
                        }
                        for p in m.params.iter() {
                            if let Some(hint) = &p.type_hint
                                && let Some(class_str) = type_hint_to_class_string(hint, None)
                            {
                                map.insert(format!("${}", p.name), class_str);
                            }
                        }
                        if let Some(body) = &m.body {
                            collect_types_stmts(
                                source,
                                body,
                                map,
                                meta,
                                method_returns,
                                cursor_byte,
                            );
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_types_stmts(source, inner, map, meta, method_returns, cursor_byte);
                }
            }
            // if ($x instanceof Foo) — narrow $x to Foo inside the then-branch
            StmtKind::If(if_stmt) => {
                // Check whether the condition is a simple `$var instanceof ClassName`.
                if let ExprKind::Binary(b) = &if_stmt.condition.kind
                    && b.op == BinaryOp::Instanceof
                    && let (ExprKind::Variable(var_name), ExprKind::Identifier(class)) =
                        (&b.left.kind, &b.right.kind)
                {
                    let var_key = format!("${}", var_name.as_str());
                    let narrowed = class
                        .as_str()
                        .trim_start_matches('\\')
                        .rsplit('\\')
                        .next()
                        .unwrap_or(class)
                        .to_string();
                    // Insert narrowed type then recurse into then-branch.
                    // The flat map keeps the last write, so code after the if-block
                    // may see the narrowed type — acceptable trade-off for a simple
                    // single-pass map.
                    map.insert(var_key, narrowed);
                }
                collect_types_stmts(
                    source,
                    std::slice::from_ref(if_stmt.then_branch),
                    map,
                    meta,
                    method_returns,
                    cursor_byte,
                );
                for elseif in if_stmt.elseif_branches.iter() {
                    collect_types_stmts(
                        source,
                        std::slice::from_ref(&elseif.body),
                        map,
                        meta,
                        method_returns,
                        cursor_byte,
                    );
                }
                if let Some(else_branch) = if_stmt.else_branch {
                    collect_types_stmts(
                        source,
                        std::slice::from_ref(else_branch),
                        map,
                        meta,
                        method_returns,
                        cursor_byte,
                    );
                }
            }

            // foreach ($arr as $item) — propagate element type from $arr[] to $item
            StmtKind::Foreach(f) => {
                if let ExprKind::Variable(arr_name) = &f.expr.kind {
                    let elem_key = format!("${}[]", arr_name.as_str());
                    if let Some(elem_type) = map.get(&elem_key).cloned()
                        && let ExprKind::Variable(val_name) = &f.value.kind
                    {
                        map.insert(format!("${}", val_name.as_str()), elem_type);
                    }
                }
                collect_types_stmts(
                    source,
                    std::slice::from_ref(f.body),
                    map,
                    meta,
                    method_returns,
                    cursor_byte,
                );
            }
            // try { ... } catch (FooException $e) { ... }
            // Map the catch variable to the first caught exception class.
            StmtKind::TryCatch(t) => {
                collect_types_stmts(source, &t.body, map, meta, method_returns, cursor_byte);
                for catch in t.catches.iter() {
                    if let Some(var_name) = &catch.var
                        && let Some(first_type) = catch.types.first()
                    {
                        let class_name = first_type
                            .to_string_repr()
                            .trim_start_matches('\\')
                            .rsplit('\\')
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if !class_name.is_empty() {
                            map.insert(format!("${}", var_name), class_name);
                        }
                    }
                    collect_types_stmts(
                        source,
                        &catch.body,
                        map,
                        meta,
                        method_returns,
                        cursor_byte,
                    );
                }
                if let Some(finally) = &t.finally {
                    collect_types_stmts(source, finally, map, meta, method_returns, cursor_byte);
                }
            }

            // static $var = expr — infer type from the default value expression.
            StmtKind::StaticVar(vars) => {
                for var in vars.iter() {
                    let var_key = format!("${}", var.name);
                    if let Some(default) = &var.default {
                        if let ExprKind::New(new_expr) = &default.kind
                            && let Some(class_name) = extract_class_name(new_expr.class)
                        {
                            map.insert(var_key.clone(), class_name);
                        }
                        if let ExprKind::Array(_) = &default.kind {
                            map.insert(var_key, "array".to_string());
                        }
                    }
                }
            }

            _ => {}
        }
    }
}

fn collect_types_expr(
    source: &str,
    expr: &php_ast::Expr<'_, '_>,
    map: &mut HashMap<String, String>,
    meta: Option<&PhpStormMeta>,
    method_returns: &[&MethodReturnsMap],
    cursor_byte: Option<u32>,
) {
    match &expr.kind {
        ExprKind::Assign(assign) => {
            if let ExprKind::Variable(var_name) = &assign.target.kind {
                // Handle ??= (null coalescing assignment): only assigns if null
                // so use or_insert (existing type takes precedence)
                if assign.op == php_ast::AssignOp::Coalesce {
                    if let ExprKind::New(new_expr) = &assign.value.kind
                        && let Some(class_name) = extract_class_name(new_expr.class)
                    {
                        map.entry(format!("${}", var_name.as_str()))
                            .or_insert(class_name);
                    }
                    collect_types_expr(
                        source,
                        assign.value,
                        map,
                        meta,
                        method_returns,
                        cursor_byte,
                    );
                    return;
                }
                if let ExprKind::New(new_expr) = &assign.value.kind
                    && let Some(class_name) = extract_class_name(new_expr.class)
                {
                    map.insert(format!("${}", var_name.as_str()), class_name);
                }
                // $result = $obj->method() — infer result type from method's return type
                if let ExprKind::MethodCall(mc) = &assign.value.kind
                    && let (ExprKind::Variable(obj_var), ExprKind::Identifier(method_name)) =
                        (&mc.object.kind, &mc.method.kind)
                    && let Some(obj_class) = map.get(&format!("${}", obj_var.as_str())).cloned()
                    && let Some(ret_type) =
                        lookup_method_return(method_returns, &obj_class, method_name.as_str())
                {
                    map.insert(format!("${}", var_name.as_str()), ret_type.to_string());
                }
                // PHPStorm meta: `$var = $obj->make(SomeClass::class)`
                if let Some(meta) = meta
                    && let Some(inferred) = infer_from_meta_method_call(assign.value, map, meta)
                {
                    map.insert(format!("${}", var_name.as_str()), inferred);
                }
                // $result = array_map(fn($x): Foo => ..., $arr) → $result[] = Foo
                if let Some(elem_type) = extract_array_callback_return_type(assign.value) {
                    map.insert(format!("${}[]", var_name.as_str()), elem_type);
                }
            }
            collect_types_expr(source, assign.value, map, meta, method_returns, cursor_byte);
        }

        // Closure::bind($fn, $obj) → $this maps to $obj's class
        ExprKind::StaticMethodCall(s) => {
            if let ExprKind::Identifier(class) = &s.class.kind
                && class.as_str() == "Closure"
                && s.method.name_str() == Some("bind")
                && let Some(obj_arg) = s.args.get(1)
                && let Some(cls) = resolve_var_type_str(&obj_arg.value, map)
            {
                map.insert("$this".to_string(), cls);
            }
        }

        // $fn->bindTo($obj) or $fn->call($obj) → $this maps to $obj's class
        ExprKind::MethodCall(m) => {
            if let ExprKind::Identifier(method) = &m.method.kind {
                let mname = method.as_str();
                if (mname == "bindTo" || mname == "call")
                    && let Some(obj_arg) = m.args.first()
                    && let Some(cls) = resolve_var_type_str(&obj_arg.value, map)
                {
                    map.insert("$this".to_string(), cls);
                }
            }
        }

        // Walk closure bodies so inner assignments are also captured
        ExprKind::Closure(c) => {
            for p in c.params.iter() {
                if let Some(hint) = &p.type_hint
                    && let TypeHintKind::Named(name) = &hint.kind
                {
                    map.insert(format!("${}", p.name), name.to_string_repr().to_string());
                }
            }
            // Snapshot captured `use` variable types from the outer scope so they
            // remain resolvable inside the closure body even if the body walk
            // encounters assignments that would shadow them.
            let use_var_snapshot: Vec<(String, String)> = c
                .use_vars
                .iter()
                .filter_map(|uv| {
                    let key = format!("${}", uv.name);
                    map.get(&key).map(|ty| (key, ty.clone()))
                })
                .collect();
            collect_types_stmts(source, &c.body, map, meta, method_returns, cursor_byte);
            // Restore captured variable types: inner assignments inside the closure
            // body should not affect the outer scope's type for completions.
            for (key, ty) in use_var_snapshot {
                map.insert(key, ty);
            }
        }

        ExprKind::ArrowFunction(af) => {
            for p in af.params.iter() {
                if let Some(hint) = &p.type_hint
                    && let TypeHintKind::Named(name) = &hint.kind
                {
                    map.insert(format!("${}", p.name), name.to_string_repr().to_string());
                }
            }
            collect_types_expr(source, af.body, map, meta, method_returns, cursor_byte);
        }

        _ => {}
    }
}

/// For `array_map`/`array_filter` calls: extract the return type of the first
/// (callback) argument if it has an explicit type hint, e.g.
/// `array_map(fn($x): Foo => $x->transform(), $arr)` → `"Foo"`.
fn extract_array_callback_return_type(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    let ExprKind::FunctionCall(call) = &expr.kind else {
        return None;
    };
    let fn_name = match &call.name.kind {
        ExprKind::Identifier(n) => n.as_str(),
        _ => return None,
    };
    if fn_name != "array_map" && fn_name != "array_filter" {
        return None;
    }
    let callback_arg = call.args.first()?;
    extract_callback_return_type(&callback_arg.value)
}

/// Extract the return-type class name from a Closure or ArrowFunction expression.
fn extract_callback_return_type(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    let hint = match &expr.kind {
        ExprKind::Closure(c) => c.return_type.as_ref()?,
        ExprKind::ArrowFunction(af) => af.return_type.as_ref()?,
        _ => return None,
    };
    if let TypeHintKind::Named(name) = &hint.kind {
        let s = name.to_string_repr();
        let base = s.trim_start_matches('\\');
        let short = base.rsplit('\\').next().unwrap_or(base);
        if short
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            return Some(short.to_string());
        }
    }
    None
}

/// Look up the class of a `$variable` expression from the current map.
fn resolve_var_type_str(
    expr: &php_ast::Expr<'_, '_>,
    map: &HashMap<String, String>,
) -> Option<String> {
    if let ExprKind::Variable(v) = &expr.kind {
        map.get(&format!("${}", v.as_str())).cloned()
    } else {
        None
    }
}

fn extract_class_name(expr: &php_ast::Expr<'_, '_>) -> Option<String> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(name.as_str().to_string()),
        _ => None,
    }
}

/// Try to infer the return type of `$obj->method(SomeClass::class)` using the
/// PHPStorm meta map.  `map` is consulted to resolve `$obj`'s class.
fn infer_from_meta_method_call(
    expr: &php_ast::Expr<'_, '_>,
    var_map: &HashMap<String, String>,
    meta: &PhpStormMeta,
) -> Option<String> {
    let ExprKind::MethodCall(m) = &expr.kind else {
        return None;
    };
    // Resolve the receiver's type.
    let receiver_class = match &m.object.kind {
        ExprKind::Variable(v) => {
            let key = format!("${}", v.as_str());
            var_map.get(&key)?.clone()
        }
        _ => return None,
    };
    // Get the method name.
    let method_name = match &m.method.kind {
        ExprKind::Identifier(n) => n.to_string(),
        _ => return None,
    };
    // Get the first argument as a class name string.
    let arg = m.args.first()?;
    let arg_str = match &arg.value.kind {
        ExprKind::String(s) => s.trim_start_matches('\\').to_string(),
        ExprKind::ClassConstAccess(c) if c.member.name_str() == Some("class") => {
            match &c.class.kind {
                ExprKind::Identifier(n) => n
                    .trim_start_matches('\\')
                    .rsplit('\\')
                    .next()
                    .unwrap_or(n)
                    .to_string(),
                _ => return None,
            }
        }
        _ => return None,
    };
    meta.resolve_return_type(&receiver_class, &method_name, &arg_str)
        .map(|s| s.to_string())
}

/// Return the direct parent class name of `class_name` in `doc`, if any.
pub fn parent_class_name(doc: &ParsedDoc, class_name: &str) -> Option<String> {
    parent_in_stmts(&doc.program().stmts, class_name)
}

fn parent_in_stmts(stmts: &[Stmt<'_, '_>], class_name: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                return c.extends.as_ref().map(|n| n.to_string_repr().to_string());
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let found @ Some(_) = parent_in_stmts(inner, class_name)
                {
                    return found;
                }
            }
            _ => {}
        }
    }
    None
}

/// All members of a named class split by kind and static-ness.
#[derive(Debug, Default)]
pub struct ClassMembers {
    /// (name, is_static)
    pub methods: Vec<(String, bool)>,
    /// (name, is_static)
    pub properties: Vec<(String, bool)>,
    /// Names of readonly properties (PHP 8.1+).
    pub readonly_properties: Vec<String>,
    pub constants: Vec<String>,
    /// Direct parent class name, if any.
    pub parent: Option<String>,
    /// Trait names used by this class (`use Foo, Bar;`).
    pub trait_uses: Vec<String>,
}

/// Return all members (methods, properties, constants) of `class_name`.
/// Also returns the direct parent class name via `ClassMembers::parent`.
pub fn members_of_class(doc: &ParsedDoc, class_name: &str) -> ClassMembers {
    let mut out = ClassMembers::default();
    out.parent = collect_members_stmts(doc.source(), &doc.program().stmts, class_name, &mut out);
    out
}

fn collect_members_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    out: &mut ClassMembers,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                // Check docblock for @property and @method tags
                if let Some(raw) = docblock_before(source, stmt.span.start) {
                    let db = parse_docblock(&raw);
                    for prop in &db.properties {
                        out.properties.push((prop.name.clone(), false));
                    }
                    for method in &db.methods {
                        out.methods.push((method.name.clone(), method.is_static));
                    }
                }
                for member in c.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            out.methods.push((m.name.to_string(), m.is_static));
                            // Constructor-promoted params become instance properties.
                            if m.name == "__construct" {
                                for p in m.params.iter() {
                                    if p.visibility.is_some() {
                                        out.properties.push((p.name.to_string(), false));
                                        // Detect `readonly` in the source text before the
                                        // param name (the AST does not expose this flag on
                                        // Param, so we scan the raw text of the param span).
                                        let param_src =
                                            &source[p.span.start as usize..p.span.end as usize];
                                        if param_src.contains("readonly") {
                                            out.readonly_properties.push(p.name.to_string());
                                        }
                                    }
                                }
                            }
                        }
                        ClassMemberKind::Property(p) => {
                            out.properties.push((p.name.to_string(), p.is_static));
                            if p.is_readonly {
                                out.readonly_properties.push(p.name.to_string());
                            }
                        }
                        ClassMemberKind::ClassConst(c) => {
                            out.constants.push(c.name.to_string());
                        }
                        ClassMemberKind::TraitUse(t) => {
                            for name in t.traits.iter() {
                                out.trait_uses.push(name.to_string_repr().to_string());
                            }
                        }
                    }
                }
                return c.extends.as_ref().map(|n| n.to_string_repr().to_string());
            }
            StmtKind::Enum(e) if e.name == class_name => {
                let is_backed = e.scalar_type.is_some();
                // Every enum instance exposes `->name`; backed enums also expose `->value`.
                out.properties.push(("name".to_string(), false));
                if is_backed {
                    out.properties.push(("value".to_string(), false));
                }
                // Built-in static methods present on every enum.
                out.methods.push(("cases".to_string(), true));
                if is_backed {
                    out.methods.push(("from".to_string(), true));
                    out.methods.push(("tryFrom".to_string(), true));
                }
                // User-declared cases, methods, and constants.
                for member in e.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Case(c) => {
                            out.constants.push(c.name.to_string());
                        }
                        EnumMemberKind::Method(m) => {
                            out.methods.push((m.name.to_string(), m.is_static));
                        }
                        EnumMemberKind::ClassConst(c) => {
                            out.constants.push(c.name.to_string());
                        }
                        _ => {}
                    }
                }
                return None; // enums have no parent class
            }
            StmtKind::Trait(t) if t.name == class_name => {
                for member in t.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            out.methods.push((m.name.to_string(), m.is_static));
                        }
                        ClassMemberKind::Property(p) => {
                            out.properties.push((p.name.to_string(), p.is_static));
                        }
                        ClassMemberKind::ClassConst(c) => {
                            out.constants.push(c.name.to_string());
                        }
                        ClassMemberKind::TraitUse(t) => {
                            for name in t.traits.iter() {
                                out.trait_uses.push(name.to_string_repr().to_string());
                            }
                        }
                    }
                }
                return None; // traits have no parent
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    let result = collect_members_stmts(source, inner, class_name, out);
                    if result.is_some() {
                        return result;
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Return the `@mixin` class names declared in `class_name`'s docblock.
pub fn mixin_classes_of(doc: &ParsedDoc, class_name: &str) -> Vec<String> {
    let source = doc.source();
    mixin_classes_in_stmts(source, &doc.program().stmts, class_name)
}

fn mixin_classes_in_stmts(source: &str, stmts: &[Stmt<'_, '_>], class_name: &str) -> Vec<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                if let Some(raw) = docblock_before(source, stmt.span.start) {
                    return parse_docblock(&raw).mixins;
                }
                return vec![];
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    let found = mixin_classes_in_stmts(source, inner, class_name);
                    if !found.is_empty() {
                        return found;
                    }
                }
            }
            _ => {}
        }
    }
    vec![]
}

/// Return the name of the class whose body contains `position`, or `None`.
pub fn enclosing_class_at(_source: &str, doc: &ParsedDoc, position: Position) -> Option<String> {
    let sv = doc.view();
    enclosing_class_in_stmts(sv, &doc.program().stmts, position)
}

fn enclosing_class_in_stmts(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    pos: Position,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return c.name.map(|n| n.to_string());
                }
            }
            StmtKind::Enum(e) => {
                let start = sv.position_of(stmt.span.start).line;
                let end = sv.position_of(stmt.span.end).line;
                if pos.line >= start && pos.line <= end {
                    return Some(e.name.to_string());
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(found) = enclosing_class_in_stmts(sv, inner, pos)
                {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

/// Return the parameter names of the function or method named `func_name`.
pub fn params_of_function(doc: &ParsedDoc, func_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_params_stmts(&doc.program().stmts, func_name, &mut out);
    out
}

/// Return the parameter names of `method_name` on class `class_name`.
/// Primarily used to offer named-argument completions for attribute constructors.
pub fn params_of_method(doc: &ParsedDoc, class_name: &str, method_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    collect_method_params_stmts(&doc.program().stmts, class_name, method_name, &mut out);
    out
}

fn collect_method_params_stmts(
    stmts: &[php_ast::Stmt<'_, '_>],
    class_name: &str,
    method_name: &str,
    out: &mut Vec<String>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(class_name) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        for p in m.params.iter() {
                            out.push(p.name.to_string());
                        }
                        return;
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_method_params_stmts(inner, class_name, method_name, out);
                }
            }
            _ => {}
        }
    }
}

/// Returns `true` if `class_name` is declared as an `enum` in `doc`.
pub fn is_enum(doc: &ParsedDoc, class_name: &str) -> bool {
    is_enum_in_stmts(&doc.program().stmts, class_name)
}

fn is_enum_in_stmts(stmts: &[Stmt<'_, '_>], name: &str) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Enum(e) if e.name == name => return true,
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && is_enum_in_stmts(inner, name)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Returns `true` if `class_name` is a *backed* enum (`enum Foo: string` /
/// `enum Foo: int`) in `doc`.  Backed enums have a `->value` property.
pub fn is_backed_enum(doc: &ParsedDoc, class_name: &str) -> bool {
    is_backed_enum_in_stmts(&doc.program().stmts, class_name)
}

fn is_backed_enum_in_stmts(stmts: &[Stmt<'_, '_>], name: &str) -> bool {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Enum(e) if e.name == name => return e.scalar_type.is_some(),
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && is_backed_enum_in_stmts(inner, name)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn collect_params_stmts(stmts: &[Stmt<'_, '_>], func_name: &str, out: &mut Vec<String>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == func_name => {
                for p in f.params.iter() {
                    out.push(p.name.to_string());
                }
                return;
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == func_name
                    {
                        for p in m.params.iter() {
                            out.push(p.name.to_string());
                        }
                        return;
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_params_stmts(inner, func_name, out);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_type_from_new_expression() {
        let src = "<?php\n$obj = new Foo();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$obj"), Some("Foo"));
    }

    #[test]
    fn unknown_variable_returns_none() {
        let src = "<?php\n$obj = new Foo();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert!(tm.get("$other").is_none());
    }

    #[test]
    fn multiple_assignments() {
        let src = "<?php\n$a = new Foo();\n$b = new Bar();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$a"), Some("Foo"));
        assert_eq!(tm.get("$b"), Some("Bar"));
    }

    #[test]
    fn later_assignment_overwrites() {
        let src = "<?php\n$a = new Foo();\n$a = new Bar();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$a"), Some("Bar"));
    }

    #[test]
    fn infers_type_from_typed_param() {
        let src = "<?php\nfunction process(Mailer $mailer): void { $mailer-> }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$mailer"), Some("Mailer"));
    }

    #[test]
    fn parent_class_name_finds_parent() {
        let src = "<?php\nclass Base {}\nclass Child extends Base {}";
        let doc = ParsedDoc::parse(src.to_string());
        assert_eq!(parent_class_name(&doc, "Child"), Some("Base".to_string()));
    }

    #[test]
    fn parent_class_name_returns_none_for_top_level() {
        let src = "<?php\nclass Base {}";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(parent_class_name(&doc, "Base").is_none());
    }

    #[test]
    fn members_of_class_includes_parent_field() {
        let src = "<?php\nclass Base {}\nclass Child extends Base {}";
        let doc = ParsedDoc::parse(src.to_string());
        let m = members_of_class(&doc, "Child");
        assert_eq!(m.parent.as_deref(), Some("Base"));
    }

    #[test]
    fn members_of_class_finds_methods() {
        let src = "<?php\nclass Calc { public function add() {} public function sub() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Calc");
        let names: Vec<&str> = members.methods.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"add"), "missing 'add'");
        assert!(names.contains(&"sub"), "missing 'sub'");
    }

    #[test]
    fn members_of_unknown_class_is_empty() {
        let src = "<?php\nclass Calc { public function add() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Unknown");
        assert!(members.methods.is_empty());
    }

    #[test]
    fn constructor_promoted_params_appear_as_properties() {
        let src = "<?php\nclass Point {\n    public function __construct(\n        public float $x,\n        public float $y,\n    ) {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Point");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            prop_names.contains(&"x"),
            "promoted param x should be a property"
        );
        assert!(
            prop_names.contains(&"y"),
            "promoted param y should be a property"
        );
    }

    #[test]
    fn promoted_readonly_params_appear_in_readonly_properties() {
        let src = "<?php\nclass User {\n    public function __construct(\n        public readonly string $name,\n        public int $age,\n    ) {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "User");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            prop_names.contains(&"name"),
            "promoted param name should be a property"
        );
        assert!(
            prop_names.contains(&"age"),
            "promoted param age should be a property"
        );
        assert!(
            members.readonly_properties.contains(&"name".to_string()),
            "readonly promoted param name should be in readonly_properties"
        );
        assert!(
            !members.readonly_properties.contains(&"age".to_string()),
            "non-readonly promoted param age should not be in readonly_properties"
        );
    }

    #[test]
    fn enum_instance_members_include_name() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Status");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            prop_names.contains(&"name"),
            "pure enum should expose ->name"
        );
        assert!(
            !prop_names.contains(&"value"),
            "pure enum should not expose ->value"
        );
    }

    #[test]
    fn backed_enum_exposes_value_and_factory_methods() {
        let src = "<?php\nenum Color: string { case Red = 'red'; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Color");
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        let method_names: Vec<&str> = members.methods.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            prop_names.contains(&"value"),
            "backed enum should expose ->value"
        );
        assert!(
            method_names.contains(&"from"),
            "backed enum should have ::from()"
        );
        assert!(
            method_names.contains(&"tryFrom"),
            "backed enum should have ::tryFrom()"
        );
        assert!(
            method_names.contains(&"cases"),
            "enum should have ::cases()"
        );
    }

    #[test]
    fn enum_cases_appear_as_constants() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Status");
        assert!(members.constants.contains(&"Active".to_string()));
        assert!(members.constants.contains(&"Inactive".to_string()));
    }

    #[test]
    fn trait_members_are_collected() {
        let src = "<?php\ntrait Logging { public function log() {} public string $logFile; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Logging");
        let method_names: Vec<&str> = members.methods.iter().map(|(n, _)| n.as_str()).collect();
        let prop_names: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(
            method_names.contains(&"log"),
            "trait method log should be collected"
        );
        assert!(
            prop_names.contains(&"logFile"),
            "trait property logFile should be collected"
        );
    }

    #[test]
    fn class_with_trait_use_lists_trait() {
        let src = "<?php\ntrait Logging { public function log() {} }\nclass App { use Logging; }";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "App");
        assert!(
            members.trait_uses.contains(&"Logging".to_string()),
            "should list used trait"
        );
    }

    #[test]
    fn var_docblock_with_explicit_varname_infers_type() {
        let src = "<?php\n/** @var Mailer $mailer */\n$mailer = $container->get('mailer');";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$mailer"),
            Some("Mailer"),
            "@var with explicit name should map the variable"
        );
    }

    #[test]
    fn var_docblock_without_varname_infers_from_assignment() {
        let src = "<?php\n/** @var Repository */\n$repo = $this->getRepository();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$repo"),
            Some("Repository"),
            "@var without name should use assignment LHS"
        );
    }

    #[test]
    fn var_docblock_does_not_map_primitive_types() {
        let src = "<?php\n/** @var string */\n$name = 'hello';";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        // Primitives (lowercase) should not be mapped as class names.
        assert!(
            tm.get("$name").is_none(),
            "primitive @var should not produce a class mapping"
        );
    }

    #[test]
    fn var_nullable_docblock_maps_to_class() {
        // `@var ?Foo $x` is now normalised to `Foo|null` by the mir parser.
        // The type_map must still infer the class name `Foo`, not `Foo|null`.
        let src = "<?php\n/** @var ?Mailer $mailer */\n$mailer = null;";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$mailer"),
            Some("Mailer"),
            "@var ?Foo should map to 'Foo', not 'Foo|null'"
        );
    }

    #[test]
    fn var_union_docblock_maps_first_class() {
        // `@var Foo|null $x` — first class-type component should be used.
        let src = "<?php\n/** @var Repository|null $repo */\n$repo = null;";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$repo"),
            Some("Repository"),
            "@var Foo|null should map to 'Foo', not 'Foo|null'"
        );
    }

    #[test]
    fn is_enum_pure() {
        let src = "<?php\nenum Suit { case Hearts; case Clubs; }";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(is_enum(&doc, "Suit"));
        assert!(!is_backed_enum(&doc, "Suit"));
    }

    #[test]
    fn is_backed_enum_string() {
        let src = "<?php\nenum Status: string { case Active = 'active'; }";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(is_enum(&doc, "Status"));
        assert!(is_backed_enum(&doc, "Status"));
    }

    #[test]
    fn is_enum_false_for_class() {
        let src = "<?php\nclass Foo {}";
        let doc = ParsedDoc::parse(src.to_string());
        assert!(!is_enum(&doc, "Foo"));
        assert!(!is_backed_enum(&doc, "Foo"));
    }

    #[test]
    fn array_map_with_typed_closure_populates_element_type() {
        let src = "<?php\n$objs = new Foo();\n$result = array_map(fn($x): Bar => $x->transform(), $objs);";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$result[]"),
            Some("Bar"),
            "array_map with typed fn callback should store element type as $result[]"
        );
    }

    #[test]
    fn foreach_propagates_array_map_element_type() {
        let src = "<?php\n$items = array_map(fn($x): Widget => $x, []);\nforeach ($items as $item) { $item-> }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$item"),
            Some("Widget"),
            "foreach over array_map result should propagate element type to loop variable"
        );
    }

    #[test]
    fn closure_use_var_type_is_available_inside_body() {
        let src = "<?php\n$svc = new PaymentService();\n$fn = function() use ($svc) { $svc->process(); };";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$svc"),
            Some("PaymentService"),
            "captured use variable should retain its outer type inside closure body"
        );
    }

    #[test]
    fn closure_use_var_inner_assignment_does_not_override_outer_type() {
        // If inside a closure we assign $svc = new Other(), the outer $svc type
        // should be restored after walking the closure body (or_insert semantics).
        let src = "<?php\n$svc = new PaymentService();\n$fn = function() use ($svc) { $svc = new OtherService(); };";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        // The snapshot restore ensures $svc retains PaymentService for the outer scope.
        assert_eq!(
            tm.get("$svc"),
            Some("PaymentService"),
            "outer type should not be overwritten by inner assignment in closure"
        );
    }

    #[test]
    fn closure_bind_maps_this_to_obj_class() {
        let src = "<?php\n$service = new Mailer();\n$fn = Closure::bind(function() {}, $service);";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$this"),
            Some("Mailer"),
            "Closure::bind with typed object should map $this to that class"
        );
    }

    #[test]
    fn instanceof_narrows_variable_type() {
        let src = "<?php\nif ($x instanceof Foo) { $x->foo(); }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$x"),
            Some("Foo"),
            "instanceof should narrow $x to Foo inside the if body"
        );
    }

    #[test]
    fn instanceof_narrows_fqn_to_short_name() {
        let src = "<?php\nif ($x instanceof App\\Services\\Mailer) { $x->send(); }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$x"),
            Some("Mailer"),
            "instanceof with FQN should narrow to short name"
        );
    }

    #[test]
    fn closure_bind_to_maps_this_to_obj_class() {
        let src = "<?php\n$svc = new Logger();\n$fn = function() {};\n$bound = $fn->bindTo($svc);";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$this"),
            Some("Logger"),
            "bindTo() should map $this to the bound object's class"
        );
    }

    #[test]
    fn param_docblock_type_inferred() {
        let src = "<?php\n/**\n * @param Mailer $mailer\n */\nfunction send($mailer) { $mailer-> }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$mailer"), Some("Mailer"));
    }

    #[test]
    fn param_docblock_does_not_override_ast_hint() {
        let src = "<?php\n/**\n * @param OtherClass $x\n */\nfunction foo(Foo $x) {}";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        // AST type hint takes precedence over docblock (AST processed after, overwrites)
        assert_eq!(tm.get("$x"), Some("Foo"));
    }

    #[test]
    fn method_chain_return_type_from_ast_hint() {
        let src = "<?php\nclass Repo {\n    public function findFirst(): User { }\n}\nclass User { public function getName(): string {} }\n$repo = new Repo();\n$user = $repo->findFirst();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$user"), Some("User"));
    }

    #[test]
    fn method_chain_return_type_from_docblock() {
        let src = "<?php\nclass Repo {\n    /** @return Product */\n    public function latest() {}\n}\n$repo = new Repo();\n$product = $repo->latest();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$product"), Some("Product"));
    }

    #[test]
    fn not_null_check_preserves_existing_type() {
        let src = "<?php\n$x = new Foo();\nif ($x !== null) { $x-> }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$x"), Some("Foo"));
    }

    #[test]
    fn self_return_type_resolves_to_class() {
        let src = "<?php\nclass Builder {\n    public function setName(string $n): self { return $this; }\n}\n$b = new Builder();\n$b2 = $b->setName('x');";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$b2"), Some("Builder"));
    }

    #[test]
    fn null_coalesce_assign_infers_type() {
        let src = "<?php\n$obj ??= new Foo();";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(tm.get("$obj"), Some("Foo"));
    }

    #[test]
    fn docblock_property_appears_in_members() {
        let src =
            "<?php\n/**\n * @property string $email\n * @property-read int $id\n */\nclass User {}";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "User");
        let props: Vec<&str> = members.properties.iter().map(|(n, _)| n.as_str()).collect();
        assert!(props.contains(&"email"));
        assert!(props.contains(&"id"));
    }

    #[test]
    fn docblock_method_appears_in_members() {
        let src = "<?php\n/**\n * @method User find(int $id)\n * @method static Builder where(string $col, mixed $val)\n */\nclass Model {}";
        let doc = ParsedDoc::parse(src.to_string());
        let members = members_of_class(&doc, "Model");
        let method_names: Vec<&str> = members.methods.iter().map(|(n, _)| n.as_str()).collect();
        assert!(method_names.contains(&"find"));
        assert!(method_names.contains(&"where"));
        let where_static = members
            .methods
            .iter()
            .find(|(n, _)| n == "where")
            .map(|(_, s)| *s);
        assert_eq!(where_static, Some(true));
    }

    #[test]
    fn union_type_param_maps_both_classes() {
        // function f(Foo|Bar $x) — both Foo and Bar should be in the union type string
        let src = "<?php\nfunction f(Foo|Bar $x) {}";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        let val = tm.get("$x").expect("$x should be in the type map");
        assert!(
            val.contains("Foo"),
            "union type should contain 'Foo', got: {}",
            val
        );
        assert!(
            val.contains("Bar"),
            "union type should contain 'Bar', got: {}",
            val
        );
    }

    #[test]
    fn nullable_param_resolves_to_class() {
        // function f(?Foo $x) — $x should map to Foo (nullable stripped)
        let src = "<?php\nfunction f(?Foo $x) {}";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$x"),
            Some("Foo"),
            "nullable type hint ?Foo should map $x to Foo"
        );
    }

    #[test]
    fn static_return_type_resolves_to_class() {
        // A method returning `: static` inside `class Builder` — result should map to `Builder`
        let src = concat!(
            "<?php\n",
            "class Builder {\n",
            "    public function build(): static { return $this; }\n",
            "}\n",
            "$b = new Builder();\n",
            "$b2 = $b->build();\n",
        );
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$b2"),
            Some("Builder"),
            "method returning :static should resolve to the enclosing class 'Builder'"
        );
    }

    #[test]
    fn null_assignment_does_not_overwrite_class() {
        // $x = new Foo(); $x = null; — $x type should stay Foo because
        // assigning null does not overwrite a known class type in the single-pass map.
        let src = "<?php\n$x = new Foo();\n$x = null;\n";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        // The single-pass type map does not treat null as a class, so the last
        // successful class assignment (Foo) persists.
        assert_eq!(
            tm.get("$x"),
            Some("Foo"),
            "$x should retain its Foo type after being assigned null"
        );
    }

    #[test]
    fn infers_type_from_assignment_inside_trait_method() {
        let src = "<?php\ntrait Builder { public function make(): void { $obj = new Widget(); } }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$obj"),
            Some("Widget"),
            "type map should walk into trait method bodies"
        );
    }

    #[test]
    fn infers_type_from_assignment_inside_enum_method() {
        let src = "<?php\nenum Color { case Red; public function make(): void { $obj = new Palette(); } }";
        let doc = ParsedDoc::parse(src.to_string());
        let tm = TypeMap::from_doc(&doc);
        assert_eq!(
            tm.get("$obj"),
            Some("Palette"),
            "type map should walk into enum method bodies"
        );
    }
}
