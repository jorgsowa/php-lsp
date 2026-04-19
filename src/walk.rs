/// AST walkers — collect all spans where a name, variable, property, function,
/// method, or class reference appears in the given statements.
use std::ops::ControlFlow;

use php_ast::{
    CatchClause, ClassMember, ClassMemberKind, EnumMember, EnumMemberKind, Expr, ExprKind, Name,
    NamespaceBody, Span, Stmt, StmtKind, TypeHint, TypeHintKind,
    visitor::{
        Visitor, walk_catch_clause, walk_class_member, walk_enum_member, walk_expr, walk_stmt,
        walk_type_hint,
    },
};

use crate::ast::str_offset;

// ── Public entry points ───────────────────────────────────────────────────────

pub fn refs_in_stmts(source: &str, stmts: &[Stmt<'_, '_>], word: &str, out: &mut Vec<Span>) {
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

/// Like `refs_in_stmts`, but also matches spans inside `use` statements.
/// Needed so that renaming a class also renames its `use` import.
pub fn refs_in_stmts_with_use(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    word: &str,
    out: &mut Vec<Span>,
) {
    refs_in_stmts(source, stmts, word, out);
    use_refs(stmts, word, out);
}

fn use_refs(stmts: &[Stmt<'_, '_>], word: &str, out: &mut Vec<Span>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) => {
                for use_item in u.uses.iter() {
                    let fqn = use_item.name.to_string_repr().into_owned();
                    let alias_match = use_item.alias.map(|a| a == word).unwrap_or(false);
                    let last_seg = fqn.rsplit('\\').next().unwrap_or(&fqn);
                    if alias_match || last_seg == word {
                        let name_span = use_item.name.span();
                        let offset = (fqn.len() - last_seg.len()) as u32;
                        let syn_span = Span {
                            start: name_span.start + offset,
                            end: name_span.start + fqn.len() as u32,
                        };
                        out.push(syn_span);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    use_refs(inner, word, out);
                }
            }
            _ => {}
        }
    }
}

// ── AllRefsVisitor ────────────────────────────────────────────────────────────

struct AllRefsVisitor<'a> {
    source: &'a str,
    word: &'a str,
    out: Vec<Span>,
}

impl AllRefsVisitor<'_> {
    fn push_name_str(&mut self, name: &str) {
        if name == self.word {
            let start = str_offset(self.source, name);
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
            StmtKind::Function(f) => self.push_name_str(f.name),
            StmtKind::Class(c) => {
                if let Some(name) = c.name {
                    self.push_name_str(name);
                }
            }
            StmtKind::Interface(i) => self.push_name_str(i.name),
            StmtKind::Trait(t) => self.push_name_str(t.name),
            StmtKind::Enum(e) => self.push_name_str(e.name),
            _ => {}
        }
        walk_stmt(self, stmt)
    }

    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        if let ClassMemberKind::Method(m) = &member.kind {
            self.push_name_str(m.name);
        }
        walk_class_member(self, member)
    }

    fn visit_enum_member(&mut self, member: &EnumMember<'arena, 'src>) -> ControlFlow<()> {
        if let EnumMemberKind::Method(m) = &member.kind {
            self.push_name_str(m.name);
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

// ── Variable rename helpers ───────────────────────────────────────────────────

/// Collect all spans where `$var_name` (the variable name WITHOUT `$`) appears as an
/// `ExprKind::Variable` within `stmts`. Stops at nested function/closure/arrow-function
/// scope boundaries so that `$x` in an inner function is not conflated with `$x` in
/// the outer function.
pub fn var_refs_in_stmts(stmts: &[Stmt<'_, '_>], var_name: &str, out: &mut Vec<Span>) {
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
    out: Vec<Span>,
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
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            // Collect matching variable references.
            ExprKind::Variable(name) => {
                if name.as_str() == self.var_name {
                    self.out.push(expr.span);
                }
                ControlFlow::Continue(())
            }
            // Stop at expression-level scope boundaries.
            ExprKind::Closure(_) | ExprKind::ArrowFunction(_) => ControlFlow::Continue(()),
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
    out: &mut Vec<Span>,
) {
    for stmt in stmts {
        if collect_in_fn_at(stmt, var_name, byte_off, out) {
            return;
        }
    }
    // Not inside any function — collect top-level
    var_refs_in_stmts(stmts, var_name, out);
}

/// Returns `true` if `stmt` is (or contains) the function/method that owns `byte_off`
/// and has populated `out` with variable + param spans for `var_name`.
fn collect_in_fn_at(
    stmt: &Stmt<'_, '_>,
    var_name: &str,
    byte_off: usize,
    out: &mut Vec<Span>,
) -> bool {
    match &stmt.kind {
        StmtKind::Function(f) => {
            if byte_off < stmt.span.start as usize || byte_off >= stmt.span.end as usize {
                return false;
            }
            // Check nested functions first.
            for inner in f.body.iter() {
                if collect_in_fn_at(inner, var_name, byte_off, out) {
                    return true;
                }
            }
            // This is the enclosing function — collect param + body refs.
            for p in f.params.iter() {
                if p.name == var_name {
                    out.push(p.span);
                }
            }
            var_refs_in_stmts(&f.body, var_name, out);
            true
        }
        StmtKind::Class(c) => {
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if byte_off < member.span.start as usize || byte_off >= member.span.end as usize
                    {
                        continue;
                    }
                    if let Some(body) = &m.body {
                        for inner in body.iter() {
                            if collect_in_fn_at(inner, var_name, byte_off, out) {
                                return true;
                            }
                        }
                        for p in m.params.iter() {
                            if p.name == var_name {
                                out.push(p.span);
                            }
                        }
                        var_refs_in_stmts(body, var_name, out);
                    }
                    return true;
                }
            }
            false
        }
        StmtKind::Trait(t) => {
            for member in t.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if byte_off < member.span.start as usize || byte_off >= member.span.end as usize
                    {
                        continue;
                    }
                    if let Some(body) = &m.body {
                        for inner in body.iter() {
                            if collect_in_fn_at(inner, var_name, byte_off, out) {
                                return true;
                            }
                        }
                        for p in m.params.iter() {
                            if p.name == var_name {
                                out.push(p.span);
                            }
                        }
                        var_refs_in_stmts(body, var_name, out);
                    }
                    return true;
                }
            }
            false
        }
        StmtKind::Enum(e) => {
            for member in e.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind {
                    if byte_off < member.span.start as usize || byte_off >= member.span.end as usize
                    {
                        continue;
                    }
                    if let Some(body) = &m.body {
                        for inner in body.iter() {
                            if collect_in_fn_at(inner, var_name, byte_off, out) {
                                return true;
                            }
                        }
                        for p in m.params.iter() {
                            if p.name == var_name {
                                out.push(p.span);
                            }
                        }
                        var_refs_in_stmts(body, var_name, out);
                    }
                    return true;
                }
            }
            false
        }
        StmtKind::Interface(i) => {
            for member in i.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if byte_off < member.span.start as usize || byte_off >= member.span.end as usize
                    {
                        continue;
                    }
                    if let Some(body) = &m.body {
                        for inner in body.iter() {
                            if collect_in_fn_at(inner, var_name, byte_off, out) {
                                return true;
                            }
                        }
                        for p in m.params.iter() {
                            if p.name == var_name {
                                out.push(p.span);
                            }
                        }
                        var_refs_in_stmts(body, var_name, out);
                    }
                    return true;
                }
            }
            false
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                for s in inner.iter() {
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

// ── Property rename helpers ───────────────────────────────────────────────────

/// Collect all spans where `prop_name` is accessed (`->prop`, `?->prop`) or
/// declared as a class/trait property, across all statements.
pub fn property_refs_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    prop_name: &str,
    out: &mut Vec<Span>,
) {
    let mut v = PropertyRefsVisitor {
        source,
        prop_name,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

struct PropertyRefsVisitor<'a> {
    source: &'a str,
    prop_name: &'a str,
    out: Vec<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for PropertyRefsVisitor<'_> {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            ExprKind::PropertyAccess(p) | ExprKind::NullsafePropertyAccess(p) => {
                let span = p.property.span;
                let name_in_src = self
                    .source
                    .get(span.start as usize..span.end as usize)
                    .unwrap_or("");
                if name_in_src == self.prop_name {
                    self.out.push(span);
                }
            }
            _ => {}
        }
        walk_expr(self, expr)
    }

    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        if let ClassMemberKind::Property(p) = &member.kind
            && p.name == self.prop_name
        {
            let offset = str_offset(self.source, p.name);
            self.out.push(Span {
                start: offset,
                end: offset + p.name.len() as u32,
            });
        }
        walk_class_member(self, member)
    }
}

// ── Function-reference walker ─────────────────────────────────────────────────

/// Collect spans where `name` is called as a free function (not a method).
/// Only matches `name(...)` calls where the callee is a bare identifier, not
/// `$obj->name()` or `Class::name()`.
pub fn function_refs_in_stmts(stmts: &[Stmt<'_, '_>], name: &str, out: &mut Vec<Span>) {
    let mut v = FunctionRefsVisitor {
        name,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

struct FunctionRefsVisitor<'a> {
    name: &'a str,
    out: Vec<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for FunctionRefsVisitor<'_> {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::FunctionCall(f) = &expr.kind
            && let ExprKind::Identifier(id) = &f.name.kind
            && id.as_str() == self.name
        {
            self.out.push(f.name.span);
        }
        walk_expr(self, expr)
    }
}

// ── Method-reference walker ───────────────────────────────────────────────────

/// Collect spans where `name` is used as a method: `->name()`, `?->name()`, `::name()`.
/// Does NOT match free function calls or class-name identifiers.
pub fn method_refs_in_stmts(stmts: &[Stmt<'_, '_>], name: &str, out: &mut Vec<Span>) {
    let mut v = MethodRefsVisitor {
        name,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

struct MethodRefsVisitor<'a> {
    name: &'a str,
    out: Vec<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for MethodRefsVisitor<'_> {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
                if let ExprKind::Identifier(id) = &m.method.kind
                    && id.as_str() == self.name
                {
                    self.out.push(m.method.span);
                }
            }
            ExprKind::StaticMethodCall(s) => {
                if s.method.name_str() == Some(self.name) {
                    self.out.push(s.method.span);
                }
            }
            _ => {}
        }
        walk_expr(self, expr)
    }
}

// ── Class-reference walker ────────────────────────────────────────────────────

/// Collect spans where `class_name` is used as a class-type reference:
/// `new ClassName`, `extends ClassName`, `implements ClassName`, type hints,
/// and `$x instanceof ClassName`. Does NOT match free function calls or
/// method names with the same spelling.
pub fn class_refs_in_stmts(stmts: &[Stmt<'_, '_>], class_name: &str, out: &mut Vec<Span>) {
    let mut v = ClassRefsVisitor {
        class_name,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

struct ClassRefsVisitor<'a> {
    class_name: &'a str,
    out: Vec<Span>,
}

impl ClassRefsVisitor<'_> {
    /// Push the span of the last segment of `name` if it matches `class_name`.
    fn collect_name<'a, 'b>(&mut self, name: &Name<'a, 'b>) {
        let repr = name.to_string_repr();
        let last = repr.rsplit('\\').next().unwrap_or(repr.as_ref());
        if last == self.class_name {
            let span = name.span();
            let offset = (repr.len() - last.len()) as u32;
            self.out.push(Span {
                start: span.start + offset,
                end: span.end,
            });
        }
    }
}

impl<'arena, 'src> Visitor<'arena, 'src> for ClassRefsVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        match &stmt.kind {
            StmtKind::Class(c) => {
                if let Some(ext) = &c.extends {
                    self.collect_name(ext);
                }
                for iface in c.implements.iter() {
                    self.collect_name(iface);
                }
            }
            StmtKind::Interface(i) => {
                for parent in i.extends.iter() {
                    self.collect_name(parent);
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt)
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            ExprKind::New(n) => {
                if let ExprKind::Identifier(id) = &n.class.kind
                    && id.rsplit('\\').next().unwrap_or(id) == self.class_name
                {
                    self.out.push(n.class.span);
                }
            }
            ExprKind::Binary(b) => {
                if let ExprKind::Identifier(id) = &b.right.kind
                    && id.rsplit('\\').next().unwrap_or(id) == self.class_name
                {
                    self.out.push(b.right.span);
                }
            }
            ExprKind::StaticMethodCall(s) => {
                if let ExprKind::Identifier(id) = &s.class.kind
                    && id.rsplit('\\').next().unwrap_or(id) == self.class_name
                {
                    self.out.push(s.class.span);
                }
            }
            ExprKind::StaticPropertyAccess(s) => {
                if let ExprKind::Identifier(id) = &s.class.kind
                    && id.rsplit('\\').next().unwrap_or(id) == self.class_name
                {
                    self.out.push(s.class.span);
                }
            }
            ExprKind::ClassConstAccess(c) => {
                if let ExprKind::Identifier(id) = &c.class.kind
                    && id.rsplit('\\').next().unwrap_or(id) == self.class_name
                {
                    self.out.push(c.class.span);
                }
            }
            _ => {}
        }
        walk_expr(self, expr)
    }

    fn visit_type_hint(&mut self, type_hint: &TypeHint<'arena, 'src>) -> ControlFlow<()> {
        if let TypeHintKind::Named(name) = &type_hint.kind {
            self.collect_name(name);
        }
        walk_type_hint(self, type_hint)
    }

    fn visit_catch_clause(&mut self, catch: &CatchClause<'arena, 'src>) -> ControlFlow<()> {
        for ty in catch.types.iter() {
            self.collect_name(ty);
        }
        walk_catch_clause(self, catch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::ParsedDoc;

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
        assert!(texts.iter().any(|&t| t == "run"));
    }

    #[test]
    fn refs_returns_empty_for_unknown_name() {
        let src = "<?php\nfunction greet() {}";
        let doc = parse(src);
        let mut out = vec![];
        refs_in_stmts(src, &doc.program().stmts, "nope", &mut out);
        assert!(out.is_empty());
    }

    // ── refs_in_stmts_with_use ───────────────────────────────────────────────

    #[test]
    fn refs_with_use_includes_use_import() {
        let src = "<?php\nuse Vendor\\Lib\\Foo;\n$x = new Foo();";
        let doc = parse(src);
        let mut out = vec![];
        refs_in_stmts_with_use(src, &doc.program().stmts, "Foo", &mut out);
        let texts = spans_to_strs(src, &out);
        // Should see the `Foo` segment in the use statement + the new Foo()
        assert!(
            texts.iter().filter(|&&t| t == "Foo").count() >= 2,
            "got: {texts:?}"
        );
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
    fn collect_scope_does_not_bleed_enum_method_into_outer_scope() {
        let src =
            "<?php\n$arg = 1;\nenum Status {\n    public function label($arg) { return $arg; }\n}";
        let doc = parse(src);
        // cursor is at the top-level $arg = 1, outside the enum
        let byte_off = src.find("$arg").unwrap();
        let mut out = vec![];
        collect_var_refs_in_scope(&doc.program().stmts, "arg", byte_off, &mut out);
        // only the top-level $arg should be found, not the enum method param
        assert_eq!(
            out.len(),
            1,
            "enum method $arg must not bleed into outer scope"
        );
    }

    // ── property_refs_in_stmts ───────────────────────────────────────────────

    #[test]
    fn property_refs_finds_declaration_and_access() {
        let src = "<?php\nclass Baz { public int $val = 0; function get() { return $this->val; } }";
        let doc = parse(src);
        let mut out = vec![];
        property_refs_in_stmts(src, &doc.program().stmts, "val", &mut out);
        // property declaration + $this->val access
        assert_eq!(out.len(), 2, "expected decl + access, got {}", out.len());
    }

    #[test]
    fn property_refs_finds_nullsafe_access() {
        let src = "<?php\n$r = $obj?->name;";
        let doc = parse(src);
        let mut out = vec![];
        property_refs_in_stmts(src, &doc.program().stmts, "name", &mut out);
        assert_eq!(out.len(), 1);
    }

    // ── function_refs_in_stmts ───────────────────────────────────────────────

    #[test]
    fn function_refs_only_matches_free_calls_not_methods() {
        let src = "<?php\nfunction run() {}\nrun();\n$obj->run();";
        let doc = parse(src);
        let mut out = vec![];
        function_refs_in_stmts(&doc.program().stmts, "run", &mut out);
        // Only the free call `run()` should match; `$obj->run()` must not.
        assert_eq!(out.len(), 1, "got: {out:?}");
    }

    // ── method_refs_in_stmts ─────────────────────────────────────────────────

    #[test]
    fn method_refs_only_matches_method_calls_not_free_functions() {
        let src = "<?php\nfunction run() {}\nrun();\n$obj->run();";
        let doc = parse(src);
        let mut out = vec![];
        method_refs_in_stmts(&doc.program().stmts, "run", &mut out);
        // Only `$obj->run()` method name span should match.
        assert_eq!(out.len(), 1, "got: {out:?}");
    }

    #[test]
    fn method_refs_finds_nullsafe_method_call() {
        let src = "<?php\n$obj?->process();";
        let doc = parse(src);
        let mut out = vec![];
        method_refs_in_stmts(&doc.program().stmts, "process", &mut out);
        assert_eq!(out.len(), 1);
    }

    // ── class_refs_in_stmts ──────────────────────────────────────────────────

    #[test]
    fn class_refs_finds_new_and_extends() {
        let src = "<?php\nclass Child extends Base {}\n$x = new Base();";
        let doc = parse(src);
        let mut out = vec![];
        class_refs_in_stmts(&doc.program().stmts, "Base", &mut out);
        assert!(out.len() >= 2, "expected extends + new, got {}", out.len());
    }

    #[test]
    fn class_refs_does_not_match_free_function_with_same_name() {
        let src = "<?php\nfunction Foo() {}\nFoo();";
        let doc = parse(src);
        let mut out = vec![];
        class_refs_in_stmts(&doc.program().stmts, "Foo", &mut out);
        assert!(
            out.is_empty(),
            "free function call must not be a class ref; got: {out:?}"
        );
    }

    #[test]
    fn class_refs_finds_type_hint_in_function_param() {
        let src = "<?php\nfunction take(MyClass $obj): MyClass { return $obj; }";
        let doc = parse(src);
        let mut out = vec![];
        class_refs_in_stmts(&doc.program().stmts, "MyClass", &mut out);
        // param type hint + return type hint
        assert_eq!(out.len(), 2, "got {out:?}");
    }
}
