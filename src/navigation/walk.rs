/// AST walkers — collect all spans where a name, variable, property, function,
/// method, or class reference appears in the given statements.
use std::ops::ControlFlow;

use php_ast::{
    Attribute, CatchClause, ClassMember, ClassMemberKind, EnumMember, EnumMemberKind, Expr,
    ExprKind, MethodDecl, Name, NamespaceBody, Span, Stmt, StmtKind, TraitUseDecl, TypeHint,
    TypeHintKind, UnaryPostfixOp, UnaryPrefixOp,
    visitor::{
        Visitor, walk_attribute, walk_catch_clause, walk_class_member, walk_enum_member, walk_expr,
        walk_stmt, walk_trait_use, walk_type_hint,
    },
};
use tower_lsp::lsp_types::DocumentHighlightKind;

use crate::document::ast::{str_offset, str_offset_in_range};

// ── Public entry points ───────────────────────────────────────────────────────

pub fn refs_in_stmts(source: &str, stmts: &[Stmt<'_, '_>], word: &str, out: &mut Vec<Span>) {
    walk_all_refs(source, stmts, word, out);
}

fn walk_all_refs(source: &str, stmts: &[Stmt<'_, '_>], word: &str, out: &mut Vec<Span>) {
    let mut v = AllRefsVisitor {
        source,
        word,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

// ── AllRefsVisitor ────────────────────────────────────────────────────────────

struct AllRefsVisitor<'a> {
    source: &'a str,
    word: &'a str,
    out: Vec<Span>,
}

impl AllRefsVisitor<'_> {
    fn push_name_str(&mut self, name: &str, stmt_span: Span) {
        if name == self.word {
            let start =
                str_offset_in_range(self.source, stmt_span, name).unwrap_or(stmt_span.start);
            self.out.push(Span {
                start,
                end: start + name.len() as u32,
            });
        }
    }
}

impl<'arena, 'src> Visitor<'arena, 'src> for AllRefsVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        match &stmt.kind {
            StmtKind::Function(f) => self.push_name_str(f.name.or_error(), stmt.span),
            StmtKind::Class(c) => {
                if let Some(name) = c.name {
                    self.push_name_str(name.or_error(), stmt.span);
                }
            }
            StmtKind::Interface(i) => self.push_name_str(i.name.or_error(), stmt.span),
            StmtKind::Trait(t) => self.push_name_str(t.name.or_error(), stmt.span),
            StmtKind::Enum(e) => self.push_name_str(e.name.or_error(), stmt.span),
            _ => {}
        }
        walk_stmt(self, stmt)
    }

    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        match &member.kind {
            ClassMemberKind::Method(m) if m.name == self.word => {
                let name_str = m.name.or_error();
                // Scope the name search to this member's own span — a
                // global `str_offset` returns the first occurrence in
                // the file, so when two classes share a method name
                // both methods would resolve to the same range.
                let start = str_offset_in_range(self.source, member.span, name_str).unwrap_or(0);
                self.out.push(Span {
                    start,
                    end: start + name_str.len() as u32,
                });
            }
            ClassMemberKind::ClassConst(cc) if cc.name == self.word => {
                let name_str = cc.name.or_error();
                let start = str_offset_in_range(self.source, member.span, name_str)
                    .unwrap_or_else(|| str_offset(self.source, name_str).unwrap_or(0));
                self.out.push(Span {
                    start,
                    end: start + name_str.len() as u32,
                });
            }
            _ => {}
        }
        walk_class_member(self, member)
    }

    fn visit_enum_member(&mut self, member: &EnumMember<'arena, 'src>) -> ControlFlow<()> {
        if let EnumMemberKind::Method(m) = &member.kind
            && m.name == self.word
        {
            let name_str = m.name.or_error();
            let start = str_offset_in_range(self.source, member.span, name_str)
                .unwrap_or(member.span.start);
            self.out.push(Span {
                start,
                end: start + name_str.len() as u32,
            });
        }
        walk_enum_member(self, member)
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::Identifier(name) = &expr.kind
            && name.as_str() == self.word
        {
            self.out.push(expr.span);
        }
        walk_expr(self, expr)
    }
}

fn var_refs_in_stmts(
    stmts: &[Stmt<'_, '_>],
    var_name: &str,
    out: &mut Vec<(Span, DocumentHighlightKind)>,
) {
    let mut v = VarRefsVisitor {
        var_name,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

struct VarRefsVisitor<'a> {
    var_name: &'a str,
    out: Vec<(Span, DocumentHighlightKind)>,
}

impl VarRefsVisitor<'_> {
    /// Mark all variable nodes in an lvalue expression as WRITE positions.
    /// Handles plain variables (`$x`), array/list destructuring (`[$a, $b]`,
    /// `list($a, $b)`), and nested destructuring (`[[$a, $b], $c]`).
    /// Falls back to `visit_expr` for other lvalue shapes (e.g. `$arr[$key]`)
    /// so their sub-expressions are still collected as READ.
    fn mark_lvalue_writes(&mut self, expr: &Expr<'_, '_>) {
        match &expr.kind {
            ExprKind::Variable(name) if name.as_str() == self.var_name => {
                self.out.push((expr.span, DocumentHighlightKind::WRITE));
            }
            ExprKind::Array(elements) => {
                for elem in elements.iter() {
                    if !matches!(elem.value.kind, ExprKind::Omit) {
                        self.mark_lvalue_writes(&elem.value);
                    }
                }
            }
            _ => {
                let _ = self.visit_expr(expr);
            }
        }
    }
}

impl<'arena, 'src> Visitor<'arena, 'src> for VarRefsVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        // Stop at scope-defining statement boundaries.
        match &stmt.kind {
            StmtKind::Function(_)
            | StmtKind::Class(_)
            | StmtKind::Trait(_)
            | StmtKind::Enum(_)
            | StmtKind::Interface(_) => ControlFlow::Continue(()),
            StmtKind::Foreach(f) => {
                // foreach key/value are write positions (being assigned)
                if let Some(key) = &f.key
                    && let ExprKind::Variable(name) = &key.kind
                    && name.as_str() == self.var_name
                {
                    self.out.push((key.span, DocumentHighlightKind::WRITE));
                }
                if let ExprKind::Variable(name) = &f.value.kind
                    && name.as_str() == self.var_name
                {
                    self.out.push((f.value.span, DocumentHighlightKind::WRITE));
                }
                // Walk the rest of the foreach
                let _ = self.visit_expr(&f.expr);
                let _ = self.visit_stmt(f.body);
                ControlFlow::Continue(())
            }
            // `global $x` pulls the global into local scope; mark as WRITE
            // rather than letting walk_stmt visit it as READ.
            StmtKind::Global(exprs) => {
                for expr in exprs.iter() {
                    if let ExprKind::Variable(name) = &expr.kind
                        && name.as_str() == self.var_name
                    {
                        self.out.push((expr.span, DocumentHighlightKind::WRITE));
                    }
                }
                ControlFlow::Continue(())
            }
            // `static $x = val` declares and initialises a persistent local;
            // walk_stmt only visits the default expression and skips the name.
            StmtKind::StaticVar(vars) => {
                for sv in vars.iter() {
                    if sv.name.or_error() == self.var_name {
                        self.out.push((sv.span, DocumentHighlightKind::WRITE));
                    }
                    if let Some(default) = &sv.default {
                        let _ = self.visit_expr(default);
                    }
                }
                ControlFlow::Continue(())
            }
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            // Collect matching variable references.
            ExprKind::Variable(name) => {
                if name.as_str() == self.var_name {
                    self.out.push((expr.span, DocumentHighlightKind::READ));
                }
                ControlFlow::Continue(())
            }
            // Assignment: target is WRITE, value is READ
            ExprKind::Assign(a) => {
                self.mark_lvalue_writes(a.target);
                // Visit value with READ kind (default)
                let _ = self.visit_expr(a.value);
                ControlFlow::Continue(())
            }
            // Pre/post increment/decrement are both read and write, but mark as WRITE
            ExprKind::UnaryPrefix(u) => {
                if matches!(
                    u.op,
                    UnaryPrefixOp::PreIncrement | UnaryPrefixOp::PreDecrement
                ) && let ExprKind::Variable(name) = &u.operand.kind
                    && name.as_str() == self.var_name
                {
                    self.out
                        .push((u.operand.span, DocumentHighlightKind::WRITE));
                    return ControlFlow::Continue(());
                }
                walk_expr(self, expr)
            }
            ExprKind::UnaryPostfix(u) => {
                if matches!(
                    u.op,
                    UnaryPostfixOp::PostIncrement | UnaryPostfixOp::PostDecrement
                ) && let ExprKind::Variable(name) = &u.operand.kind
                    && name.as_str() == self.var_name
                {
                    self.out
                        .push((u.operand.span, DocumentHighlightKind::WRITE));
                    return ControlFlow::Continue(());
                }
                walk_expr(self, expr)
            }
            // Closures are scope boundaries, but arrow functions auto-capture outer variables.
            ExprKind::Closure(c) => {
                // Before stopping, collect variables from the closure's use($x) clause.
                for use_var in c.use_vars.iter() {
                    if use_var.name == self.var_name {
                        self.out.push((use_var.span, DocumentHighlightKind::READ));
                    }
                }
                ControlFlow::Continue(())
            }
            // Arrow functions auto-capture and should be traversed — unless
            // the arrow function's own parameter shadows `var_name`, in
            // which case its body refers to a different variable and must
            // not be merged into the outer variable's highlight group.
            ExprKind::ArrowFunction(af) => {
                if af.params.iter().any(|p| p.name == self.var_name) {
                    ControlFlow::Continue(())
                } else {
                    walk_expr(self, expr)
                }
            }
            _ => walk_expr(self, expr),
        }
    }
}

/// Collect all `$var_name` spans within the innermost function/method scope
/// that contains `byte_off`. If `byte_off` is not inside any function, collects
/// from the top-level stmts (respecting scope boundaries). Also collects the
/// parameter declaration span when the variable is a parameter of the scope.
pub fn collect_var_refs_in_scope(
    stmts: &[Stmt<'_, '_>],
    var_name: &str,
    byte_off: usize,
    out: &mut Vec<(Span, DocumentHighlightKind)>,
) {
    for stmt in stmts {
        if collect_in_fn_at(stmt, var_name, byte_off, out) {
            return;
        }
    }
    // Not inside any function — collect top-level
    var_refs_in_stmts(stmts, var_name, out);
}

/// Returns `true` if the cursor at `byte_off` falls within `m`'s span, collecting
/// variable references from `m`'s body and parameters into `out`.
fn collect_method_scope(
    m: &MethodDecl<'_, '_>,
    member_span: Span,
    var_name: &str,
    byte_off: usize,
    out: &mut Vec<(Span, DocumentHighlightKind)>,
) -> bool {
    if byte_off < member_span.start as usize || byte_off >= member_span.end as usize {
        return false;
    }
    if let Some(body) = &m.body {
        for inner in body.stmts.iter() {
            if collect_in_fn_at(inner, var_name, byte_off, out) {
                return true;
            }
        }
        var_refs_in_stmts(&body.stmts, var_name, out);
    }
    for p in m.params.iter() {
        if p.name == var_name {
            out.push((p.span, DocumentHighlightKind::WRITE));
        }
    }
    true
}

/// Search `members` for the method whose span contains `byte_off` and collect
/// variable references for `var_name` from it. Returns `true` when found.
fn collect_in_class_members(
    members: &[ClassMember<'_, '_>],
    var_name: &str,
    byte_off: usize,
    out: &mut Vec<(Span, DocumentHighlightKind)>,
) -> bool {
    for member in members {
        if let ClassMemberKind::Method(m) = &member.kind
            && collect_method_scope(m, member.span, var_name, byte_off, out)
        {
            return true;
        }
    }
    false
}

/// Returns `true` if `stmt` is (or contains) the function/method that owns `byte_off`
/// and has populated `out` with variable + param spans for `var_name`.
fn collect_in_fn_at(
    stmt: &Stmt<'_, '_>,
    var_name: &str,
    byte_off: usize,
    out: &mut Vec<(Span, DocumentHighlightKind)>,
) -> bool {
    match &stmt.kind {
        StmtKind::Function(f) => {
            if byte_off < stmt.span.start as usize || byte_off >= stmt.span.end as usize {
                return false;
            }
            for inner in f.body.stmts.iter() {
                if collect_in_fn_at(inner, var_name, byte_off, out) {
                    return true;
                }
            }
            for p in f.params.iter() {
                if p.name == var_name {
                    out.push((p.span, DocumentHighlightKind::WRITE));
                }
            }
            var_refs_in_stmts(&f.body.stmts, var_name, out);
            true
        }
        StmtKind::Class(c) => collect_in_class_members(&c.body.members, var_name, byte_off, out),
        StmtKind::Trait(t) => collect_in_class_members(&t.body.members, var_name, byte_off, out),
        StmtKind::Interface(i) => {
            collect_in_class_members(&i.body.members, var_name, byte_off, out)
        }
        StmtKind::Enum(e) => {
            for member in e.body.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind
                    && collect_method_scope(m, member.span, var_name, byte_off, out)
                {
                    return true;
                }
            }
            false
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                for s in inner.stmts.iter() {
                    if collect_in_fn_at(s, var_name, byte_off, out) {
                        return true;
                    }
                }
            }
            false
        }
        _ => false,
    }
}

// ── Class-reference walker ────────────────────────────────────────────────────

/// Collect every class-typed name reference (extends, implements, new,
/// instanceof, type hints, static method/property/const access, catch types).
/// Each entry is the source-spelling of the reference (`Foo`, `Sub\Foo`, or
/// `\Foo`) — callers apply namespace/use resolution to obtain an FQN.
///
/// Returned vec is sorted + de-duplicated by exact spelling.
pub fn all_class_ref_names_in_stmts(stmts: &[Stmt<'_, '_>]) -> Vec<String> {
    let mut v = AllClassRefsVisitor { out: Vec::new() };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    v.out.sort_unstable();
    v.out.dedup();
    v.out
}

struct AllClassRefsVisitor {
    out: Vec<String>,
}

impl AllClassRefsVisitor {
    fn push_name(&mut self, name: &Name<'_, '_>) {
        self.out.push(name.to_string_repr().into_owned());
    }

    fn push_id(&mut self, id: &str) {
        self.out.push(id.to_string());
    }
}

impl<'arena, 'src> Visitor<'arena, 'src> for AllClassRefsVisitor {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        match &stmt.kind {
            StmtKind::Class(c) => {
                if let Some(ext) = &c.extends {
                    self.push_name(ext);
                }
                for iface in c.implements.iter() {
                    self.push_name(iface);
                }
            }
            StmtKind::Interface(i) => {
                for parent in i.extends.iter() {
                    self.push_name(parent);
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt)
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            ExprKind::New(n) => {
                if let ExprKind::Identifier(id) = &n.class.kind {
                    self.push_id(id);
                }
            }
            ExprKind::AnonymousClass(c) => {
                if let Some(ext) = &c.extends {
                    self.push_name(ext);
                }
                for iface in c.implements.iter() {
                    self.push_name(iface);
                }
            }
            ExprKind::Binary(b) => {
                // `$x instanceof Foo` — parser models this as a Binary expr
                // whose right-hand side is an Identifier.
                if let ExprKind::Identifier(id) = &b.right.kind {
                    self.push_id(id);
                }
            }
            ExprKind::StaticMethodCall(s) => {
                if let ExprKind::Identifier(id) = &s.class.kind {
                    self.push_id(id);
                }
            }
            ExprKind::StaticPropertyAccess(s) => {
                if let ExprKind::Identifier(id) = &s.class.kind {
                    self.push_id(id);
                }
            }
            ExprKind::ClassConstAccess(c) => {
                if let ExprKind::Identifier(id) = &c.class.kind {
                    self.push_id(id);
                }
            }
            _ => {}
        }
        walk_expr(self, expr)
    }

    fn visit_attribute(&mut self, attribute: &Attribute<'arena, 'src>) -> ControlFlow<()> {
        self.push_name(&attribute.name);
        walk_attribute(self, attribute)
    }

    fn visit_type_hint(&mut self, type_hint: &TypeHint<'arena, 'src>) -> ControlFlow<()> {
        match &type_hint.kind {
            TypeHintKind::Named(name) => {
                self.push_name(name);
                walk_type_hint(self, type_hint)
            }
            TypeHintKind::Nullable(_) => walk_type_hint(self, type_hint),
            TypeHintKind::Union(types) => {
                for inner in types.iter() {
                    let _ = self.visit_type_hint(inner);
                }
                ControlFlow::Continue(())
            }
            TypeHintKind::Intersection(types) => {
                for inner in types.iter() {
                    let _ = self.visit_type_hint(inner);
                }
                ControlFlow::Continue(())
            }
            TypeHintKind::Keyword(_, _) => ControlFlow::Continue(()),
        }
    }

    fn visit_catch_clause(&mut self, catch: &CatchClause<'arena, 'src>) -> ControlFlow<()> {
        for ty in catch.types.iter() {
            self.push_name(ty);
        }
        walk_catch_clause(self, catch)
    }

    fn visit_trait_use(&mut self, trait_use: &TraitUseDecl<'arena, 'src>) -> ControlFlow<()> {
        for name in trait_use.traits.iter() {
            self.push_name(name);
        }
        walk_trait_use(self, trait_use)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::ast::ParsedDoc;

    /// Return all substrings of `source` at the given spans.
    fn spans_to_strs<'a>(source: &'a str, spans: &[Span]) -> Vec<&'a str> {
        spans
            .iter()
            .map(|s| &source[s.start as usize..s.end as usize])
            .collect()
    }

    fn parse(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    // ── refs_in_stmts ────────────────────────────────────────────────────────

    #[test]
    fn refs_finds_function_declaration_and_call() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let doc = parse(src);
        let mut out = vec![];
        refs_in_stmts(src, &doc.program().stmts, "greet", &mut out);
        let texts = spans_to_strs(src, &out);
        assert!(texts.contains(&"greet"), "expected function decl name");
        assert_eq!(texts.iter().filter(|&&t| t == "greet").count(), 2);
    }

    #[test]
    fn refs_finds_class_declaration_and_new() {
        let src = "<?php\nclass Foo {}\n$x = new Foo();";
        let doc = parse(src);
        let mut out = vec![];
        refs_in_stmts(src, &doc.program().stmts, "Foo", &mut out);
        let texts = spans_to_strs(src, &out);
        assert!(texts.iter().all(|&t| t == "Foo"));
        assert_eq!(texts.len(), 2);
    }

    #[test]
    fn refs_finds_method_declaration_inside_class() {
        let src = "<?php\nclass Bar { function run() { $this->run(); } }";
        let doc = parse(src);
        let mut out = vec![];
        refs_in_stmts(src, &doc.program().stmts, "run", &mut out);
        let texts = spans_to_strs(src, &out);
        // method decl name + method call name both appear
        assert!(texts.contains(&"run"));
    }

    #[test]
    fn refs_without_use_misses_use_import() {
        let src = "<?php\nuse Vendor\\Lib\\Foo;\n$x = new Foo();";
        let doc = parse(src);
        let mut out = vec![];
        refs_in_stmts(src, &doc.program().stmts, "Foo", &mut out);
        let texts = spans_to_strs(src, &out);
        // refs_in_stmts does NOT walk use statements
        assert!(
            texts.iter().filter(|&&t| t == "Foo").count() < 2,
            "refs_in_stmts should not include use import; got: {texts:?}"
        );
    }

    // ── var_refs_in_stmts ────────────────────────────────────────────────────

    #[test]
    fn var_refs_finds_variable_in_assignment_and_echo() {
        let src = "<?php\n$x = 1;\necho $x;";
        let doc = parse(src);
        let mut out = vec![];
        var_refs_in_stmts(&doc.program().stmts, "x", &mut out);
        assert_eq!(out.len(), 2, "expected $x in assignment and echo");
    }

    #[test]
    fn var_refs_respects_function_scope_boundary() {
        // $x inside the nested function is a separate scope — must not be collected.
        let src = "<?php\n$x = 1;\nfunction inner() { $x = 2; }";
        let doc = parse(src);
        let mut out = vec![];
        var_refs_in_stmts(&doc.program().stmts, "x", &mut out);
        // Only the top-level $x = 1; should be found (function is a scope boundary).
        assert_eq!(out.len(), 1, "inner $x must not cross scope boundary");
    }

    #[test]
    fn var_refs_traverses_if_while_for_foreach() {
        let src = "<?php\n$x = 0;\nif ($x) { $x++; }\nwhile ($x > 0) { $x--; }\nfor ($x = 0; $x < 3; $x++) {}\nforeach ([$x] as $v) {}";
        let doc = parse(src);
        let mut out = vec![];
        var_refs_in_stmts(&doc.program().stmts, "x", &mut out);
        assert!(
            out.len() >= 5,
            "expected multiple $x refs, got {}",
            out.len()
        );
    }

    #[test]
    fn var_refs_does_not_cross_closure_boundary() {
        let src = "<?php\n$x = 1;\n$f = function() { $x = 2; };";
        let doc = parse(src);
        let mut out = vec![];
        var_refs_in_stmts(&doc.program().stmts, "x", &mut out);
        // Closure is a scope boundary — inner $x not collected.
        assert_eq!(
            out.len(),
            1,
            "closure $x must not be collected by outer scope walk"
        );
    }

    #[test]
    fn var_refs_array_destructuring_lhs_is_write() {
        // [$a, $b] = [1, 2] — $a and $b on the LHS are WRITE positions.
        // var_refs_in_stmts stops at function scope boundaries, so use
        // collect_var_refs_in_scope with a byte offset inside the function body.
        let src = "<?php\nfunction f() { [$a, $b] = [1, 2]; echo $a + $b; }";
        let doc = parse(src);
        let byte_off = src.find("[$a").unwrap();
        let mut out = vec![];
        collect_var_refs_in_scope(&doc.program().stmts, "a", byte_off, &mut out);
        let writes: Vec<_> = out
            .iter()
            .filter(|(_, k)| *k == DocumentHighlightKind::WRITE)
            .collect();
        assert_eq!(
            writes.len(),
            1,
            "expected exactly 1 WRITE for $a in destructuring: {out:?}"
        );
        let reads: Vec<_> = out
            .iter()
            .filter(|(_, k)| *k == DocumentHighlightKind::READ)
            .collect();
        assert_eq!(
            reads.len(),
            1,
            "expected exactly 1 READ for $a in echo: {out:?}"
        );
    }

    #[test]
    fn var_refs_global_declaration_is_write() {
        // global $cfg — the declaration must be WRITE.
        // var_refs_in_stmts stops at function scope boundaries, so use
        // collect_var_refs_in_scope with a byte offset inside the function body.
        let src = "<?php\nfunction f() { global $cfg; echo $cfg; }";
        let doc = parse(src);
        let byte_off = src.find("global").unwrap();
        let mut out = vec![];
        collect_var_refs_in_scope(&doc.program().stmts, "cfg", byte_off, &mut out);
        let writes: Vec<_> = out
            .iter()
            .filter(|(_, k)| *k == DocumentHighlightKind::WRITE)
            .collect();
        assert_eq!(writes.len(), 1, "global $cfg must be WRITE: {out:?}");
    }

    #[test]
    fn var_refs_static_declaration_is_write_and_collected() {
        // static $n = 0 — declaration must be WRITE; $n++ also WRITE; return $n READ.
        // var_refs_in_stmts stops at function scope boundaries, so use
        // collect_var_refs_in_scope with a byte offset inside the function body.
        let src = "<?php\nfunction counter() { static $n = 0; $n++; return $n; }";
        let doc = parse(src);
        let byte_off = src.find("static").unwrap();
        let mut out = vec![];
        collect_var_refs_in_scope(&doc.program().stmts, "n", byte_off, &mut out);
        let writes: Vec<_> = out
            .iter()
            .filter(|(_, k)| *k == DocumentHighlightKind::WRITE)
            .collect();
        // static $n = 0  → WRITE, $n++ → WRITE
        assert!(
            writes.len() >= 2,
            "static $n and $n++ must both be WRITE: {out:?}"
        );
        let total = out.len();
        // static $n, $n++, return $n → at least 3 occurrences
        assert!(
            total >= 3,
            "expected at least 3 occurrences of $n, got {total}: {out:?}"
        );
    }

    // ── collect_var_refs_in_scope ────────────────────────────────────────────

    #[test]
    fn collect_scope_finds_var_inside_function() {
        let src = "<?php\nfunction foo($x) { return $x + 1; }";
        let doc = parse(src);
        // byte_off somewhere inside the function body
        let byte_off = src.find("return").unwrap();
        let mut out = vec![];
        collect_var_refs_in_scope(&doc.program().stmts, "x", byte_off, &mut out);
        // Should find the param span and the $x in return
        assert!(
            out.len() >= 2,
            "expected param + body ref, got {}",
            out.len()
        );
    }

    #[test]
    fn collect_scope_top_level_when_no_function() {
        let src = "<?php\n$x = 1;\necho $x;";
        let doc = parse(src);
        let byte_off = src.find("echo").unwrap();
        let mut out = vec![];
        collect_var_refs_in_scope(&doc.program().stmts, "x", byte_off, &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn collect_scope_finds_var_inside_enum_method() {
        let src = "<?php\nenum Status {\n    public function label($arg) { return $arg; }\n}";
        let doc = parse(src);
        let byte_off = src.find("return").unwrap();
        let mut out = vec![];
        collect_var_refs_in_scope(&doc.program().stmts, "arg", byte_off, &mut out);
        assert!(
            out.len() >= 2,
            "expected param + body ref in enum method, got {}",
            out.len()
        );
    }

    #[test]
    fn all_class_refs_collects_extends_and_implements() {
        let src = "<?php\nclass A extends B implements C, D {}";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert_eq!(out, vec!["B", "C", "D"]);
    }

    #[test]
    fn all_class_refs_collects_interface_extends() {
        let src = "<?php\ninterface I extends J, K {}";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert_eq!(out, vec!["J", "K"]);
    }

    #[test]
    fn all_class_refs_collects_new_bare_and_fqn() {
        let src = "<?php\n$a = new Local();\n$b = new \\Vendor\\Pkg\\Cls();";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(out.contains(&"Local".to_string()));
        assert!(out.contains(&"\\Vendor\\Pkg\\Cls".to_string()));
    }

    #[test]
    fn all_class_refs_collects_instanceof() {
        let src = "<?php\nif ($x instanceof MyClass) {}";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(out.contains(&"MyClass".to_string()));
    }

    #[test]
    fn all_class_refs_collects_static_call_property_const() {
        let src = "<?php\nA::method();\nB::$prop;\nC::CONST;\n$x = D::class;";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(out.contains(&"A".to_string()), "A::method() — got {out:?}");
        assert!(out.contains(&"B".to_string()), "B::$prop — got {out:?}");
        assert!(out.contains(&"C".to_string()), "C::CONST — got {out:?}");
        assert!(out.contains(&"D".to_string()), "D::class — got {out:?}");
    }

    #[test]
    fn all_class_refs_collects_type_hints_in_all_positions() {
        let src = "<?php\nclass C {\n    public P $prop;\n    public function f(Q $q): R { return $q; }\n}";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(
            out.contains(&"P".to_string()),
            "property type — got {out:?}"
        );
        assert!(out.contains(&"Q".to_string()), "param type — got {out:?}");
        assert!(out.contains(&"R".to_string()), "return type — got {out:?}");
    }

    #[test]
    fn all_class_refs_collects_catch_types() {
        let src = "<?php\ntry {} catch (FirstException | SecondException $e) {}";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(out.contains(&"FirstException".to_string()));
        assert!(out.contains(&"SecondException".to_string()));
    }

    #[test]
    fn all_class_refs_does_not_collect_free_function_calls_or_method_names() {
        let src = "<?php\nrun();\n$obj->run();";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(
            !out.contains(&"run".to_string()),
            "function call / method must not be a class ref; got {out:?}"
        );
    }

    #[test]
    fn all_class_refs_collects_trait_use_in_class() {
        let src = "<?php\nclass C {\n    use TraitOne, TraitTwo;\n}";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(out.contains(&"TraitOne".to_string()), "got {out:?}");
        assert!(out.contains(&"TraitTwo".to_string()), "got {out:?}");
    }

    #[test]
    fn all_class_refs_collects_trait_use_in_enum() {
        let src = "<?php\nenum E: int {\n    use TraitEnum;\n    case A = 1;\n}";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(out.contains(&"TraitEnum".to_string()), "got {out:?}");
    }

    #[test]
    fn all_class_refs_deduplicates() {
        let src = "<?php\n$a = new X();\n$b = new X();\n$c instanceof X;";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert_eq!(out.iter().filter(|s| s == &"X").count(), 1);
    }

    #[test]
    fn all_class_refs_collects_attribute_names() {
        let src = "<?php\n#[MyAttr]\nclass Foo {}\n#[ORM\\Entity]\nclass Bar {}";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(
            out.contains(&"MyAttr".to_string()),
            "simple attribute — got {out:?}"
        );
        assert!(
            out.contains(&"ORM\\Entity".to_string()),
            "qualified attribute — got {out:?}"
        );
    }

    #[test]
    fn all_class_refs_collects_anonymous_class_extends_and_implements() {
        let src = "<?php\n$x = new class extends Base implements Countable {};";
        let doc = parse(src);
        let out = all_class_ref_names_in_stmts(&doc.program().stmts);
        assert!(
            out.contains(&"Base".to_string()),
            "anon class extends — got {out:?}"
        );
        assert!(
            out.contains(&"Countable".to_string()),
            "anon class implements — got {out:?}"
        );
    }
}
