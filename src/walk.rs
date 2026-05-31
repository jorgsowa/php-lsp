/// AST walkers — collect all spans where a name, variable, property, function,
/// method, or class reference appears in the given statements.
use std::collections::HashSet;
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

use crate::ast::{str_offset, str_offset_in_range};
use crate::util::fqn_short_name;

// ── Public entry points ───────────────────────────────────────────────────────

pub fn refs_in_stmts(source: &str, stmts: &[Stmt<'_, '_>], word: &str, out: &mut Vec<Span>) {
    walk_all_refs(source, stmts, word, false, out);
}

/// Like `refs_in_stmts`, but also matches spans inside `use` statements.
/// Needed so that renaming a class also renames its `use` import.
pub fn refs_in_stmts_with_use(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    word: &str,
    out: &mut Vec<Span>,
) {
    walk_all_refs(source, stmts, word, true, out);
}

fn walk_all_refs(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    word: &str,
    include_use: bool,
    out: &mut Vec<Span>,
) {
    let mut v = AllRefsVisitor {
        source,
        word,
        include_use,
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
    include_use: bool,
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
            StmtKind::Function(f) => self.push_name_str(&f.name.to_string(), stmt.span),
            StmtKind::Class(c) => {
                if let Some(name) = c.name {
                    self.push_name_str(&name.to_string(), stmt.span);
                }
            }
            StmtKind::Interface(i) => self.push_name_str(&i.name.to_string(), stmt.span),
            StmtKind::Trait(t) => self.push_name_str(&t.name.to_string(), stmt.span),
            StmtKind::Enum(e) => self.push_name_str(&e.name.to_string(), stmt.span),
            StmtKind::Use(u) if self.include_use => {
                for use_item in u.uses.iter() {
                    let fqn = use_item.name.to_string_repr().into_owned();
                    if let Some(alias) = use_item.alias {
                        // If there's an alias and it matches, emit the alias span (not the FQN)
                        if alias == self.word {
                            // Find the position of the alias in the source
                            if let Some(offset) = str_offset(self.source, alias) {
                                self.out.push(Span {
                                    start: offset,
                                    end: offset + alias.len() as u32,
                                });
                            }
                        }
                    } else {
                        // No alias: check if the last segment of FQN matches
                        let last_seg = fqn_short_name(&fqn);
                        if last_seg == self.word {
                            let name_span = use_item.name.span();
                            let offset = (fqn.len() - last_seg.len()) as u32;
                            self.out.push(Span {
                                start: name_span.start + offset,
                                end: name_span.start + fqn.len() as u32,
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt)
    }

    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        match &member.kind {
            ClassMemberKind::Method(m) if m.name == self.word => {
                let name_str = m.name.to_string();
                // Scope the name search to this member's own span — a
                // global `str_offset` returns the first occurrence in
                // the file, so when two classes share a method name
                // both methods would resolve to the same range.
                let start = str_offset_in_range(self.source, member.span, &name_str).unwrap_or(0);
                self.out.push(Span {
                    start,
                    end: start + name_str.len() as u32,
                });
            }
            ClassMemberKind::ClassConst(cc) if cc.name == self.word => {
                let name_str = cc.name.to_string();
                let start = str_offset_in_range(self.source, member.span, &name_str)
                    .unwrap_or_else(|| str_offset(self.source, &name_str).unwrap_or(0));
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
        if let EnumMemberKind::Method(m) = &member.kind {
            // For enum members, we don't have a statement span, so we'll search the entire source
            let start = str_offset(self.source, &m.name.to_string()).unwrap_or(0);
            if m.name == self.word {
                self.out.push(Span {
                    start,
                    end: start + m.name.to_string().len() as u32,
                });
            }
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
pub fn var_refs_in_stmts(
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
                // Visit target with WRITE kind
                if let ExprKind::Variable(name) = &a.target.kind {
                    if name.as_str() == self.var_name {
                        self.out.push((a.target.span, DocumentHighlightKind::WRITE));
                    }
                } else {
                    let _ = self.visit_expr(a.target);
                }
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
            // Arrow functions auto-capture and should be traversed.
            ExprKind::ArrowFunction(_) => walk_expr(self, expr),
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
            // Static property access: Class::$prop, self::$prop, parent::$prop.
            // The member expression is a Variable whose span includes the `$` sigil;
            // we skip it (+1) so the emitted span covers only the name itself,
            // matching the convention used by instance-property access above.
            ExprKind::StaticPropertyAccess(s) => {
                if let ExprKind::Identifier(name) = &s.member.kind
                    && name.as_str() == self.prop_name
                    && s.member.span.start + 1 < s.member.span.end
                {
                    self.out.push(Span {
                        start: s.member.span.start + 1,
                        end: s.member.span.end,
                    });
                }
            }
            _ => {}
        }
        walk_expr(self, expr)
    }

    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        match &member.kind {
            ClassMemberKind::Property(p) if p.name == self.prop_name => {
                let offset = str_offset(self.source, &p.name.to_string()).unwrap_or(0);
                self.out.push(Span {
                    start: offset,
                    end: offset + p.name.to_string().len() as u32,
                });
            }
            // Constructor-promoted parameters act as property declarations.
            ClassMemberKind::Method(m) if m.name == "__construct" => {
                for p in m.params.iter() {
                    if p.visibility.is_some() && p.name == self.prop_name {
                        let offset = str_offset(self.source, &p.name.to_string()).unwrap_or(0);
                        self.out.push(Span {
                            start: offset,
                            end: offset + p.name.to_string().len() as u32,
                        });
                    }
                }
            }
            _ => {}
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
            ExprKind::StaticMethodCall(s) if s.method.name_str() == Some(self.name) => {
                self.out.push(s.method.span);
            }
            _ => {}
        }
        walk_expr(self, expr)
    }
}

// ── Class-constant-reference walker ──────────────────────────────────────────

/// Collect all spans where `const_name` is declared or accessed as a class
/// constant (`Class::CONST`, `self::CONST`, `parent::CONST`, `static::CONST`).
///
/// `class_filter` — when `Some`, only access expressions whose class part
/// matches the given short name (or is `self`/`parent`/`static`, or is a class
/// that directly extends the owning class in the same file) are included.
/// Pass the owning class name to avoid matching same-named constants elsewhere.
pub fn constant_refs_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    const_name: &str,
    class_filter: Option<&str>,
    out: &mut Vec<Span>,
) {
    // Pre-build the set of allowed class names: the owner itself plus any class
    // in this file that directly `extends` the owner (they inherit the constant).
    let allowed: Option<HashSet<String>> = class_filter.map(|owner| {
        let mut set = HashSet::new();
        set.insert(owner.to_string());
        for stmt in stmts {
            if let StmtKind::Class(c) = &stmt.kind
                && let Some(extends) = &c.extends
                && extends.to_string_repr() == owner
                && let Some(name) = c.name
            {
                set.insert(name.to_string());
            }
        }
        set
    });
    let mut v = ConstantRefsVisitor {
        source,
        const_name,
        allowed: allowed.as_ref(),
        current_class: None,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

struct ConstantRefsVisitor<'a> {
    source: &'a str,
    const_name: &'a str,
    /// When `Some`, only include access sites where the class expression matches
    /// a name in this set or is `self`/`parent`/`static`.
    allowed: Option<&'a HashSet<String>>,
    /// The short name of the class currently being visited; used to filter
    /// `ClassConst` declaration spans to the owning class only.
    current_class: Option<String>,
    out: Vec<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for ConstantRefsVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        let class_name: Option<String> = match &stmt.kind {
            StmtKind::Class(c) => c.name.map(|n| n.to_string()),
            StmtKind::Interface(i) => Some(i.name.to_string()),
            StmtKind::Trait(t) => Some(t.name.to_string()),
            StmtKind::Enum(e) => Some(e.name.to_string()),
            _ => {
                return walk_stmt(self, stmt);
            }
        };
        let prev = self.current_class.take();
        self.current_class = class_name;
        let r = walk_stmt(self, stmt);
        self.current_class = prev;
        r
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::ClassConstAccess(s) = &expr.kind
            && let ExprKind::Identifier(name) = &s.member.kind
            && name.as_str() == self.const_name
        {
            let include = self.allowed.is_none_or(|allowed| {
                if let ExprKind::Identifier(class_id) = &s.class.kind {
                    let cn = class_id.as_str();
                    matches!(cn, "self" | "parent" | "static") || allowed.contains(cn)
                } else {
                    true
                }
            });
            if include {
                self.out.push(s.member.span);
            }
        }
        walk_expr(self, expr)
    }

    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        if let ClassMemberKind::ClassConst(c) = &member.kind
            && c.name == self.const_name
        {
            let class_ok = self.allowed.is_none_or(|allowed| {
                self.current_class
                    .as_deref()
                    .is_none_or(|cls| allowed.contains(cls))
            });
            if class_ok {
                let name = c.name.to_string();
                let start = str_offset_in_range(self.source, member.span, &name)
                    .unwrap_or_else(|| str_offset(self.source, &name).unwrap_or(0));
                self.out.push(Span {
                    start,
                    end: start + name.len() as u32,
                });
            }
        }
        walk_class_member(self, member)
    }

    fn visit_enum_member(&mut self, member: &EnumMember<'arena, 'src>) -> ControlFlow<()> {
        if let EnumMemberKind::ClassConst(c) = &member.kind
            && c.name == self.const_name
        {
            let class_ok = self.allowed.is_none_or(|allowed| {
                self.current_class
                    .as_deref()
                    .is_none_or(|cls| allowed.contains(cls))
            });
            if class_ok {
                let name = c.name.to_string();
                let start = str_offset_in_range(self.source, member.span, &name)
                    .unwrap_or_else(|| str_offset(self.source, &name).unwrap_or(0));
                self.out.push(Span {
                    start,
                    end: start + name.len() as u32,
                });
            }
        }
        walk_enum_member(self, member)
    }
}

// ── Global constant reference walker ─────────────────────────────────────────

/// Collect spans where `const_name` is used as a global/namespace-level constant.
/// Matches bare identifiers (`MAX_SIZE`) and, when `const_fqn` is supplied,
/// also qualified names (`\Config\DB_HOST`, `Config\DB_HOST`). Emits the span
/// of just the constant name portion (not any namespace qualifier prefix).
///
/// Only matches identifiers in value positions — function-call callees, class
/// names in `new` / static access / `instanceof`, and class-const member names
/// are excluded.
///
/// Also emits the declaration span when the name appears in a `StmtKind::Const`
/// item (top-level or inside a braced namespace).
pub fn global_constant_refs_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    const_name: &str,
    const_fqn: Option<&str>,
    out: &mut Vec<Span>,
) {
    let mut v = GlobalConstRefsVisitor {
        source,
        const_name,
        const_fqn,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

struct GlobalConstRefsVisitor<'a> {
    source: &'a str,
    const_name: &'a str,
    /// FQN of the constant (e.g. `"Config\\DB_HOST"`). When set, also matches
    /// `\Config\DB_HOST` and `Config\DB_HOST` in identifier position, emitting
    /// a span pointing at just the `DB_HOST` portion.
    const_fqn: Option<&'a str>,
    out: Vec<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for GlobalConstRefsVisitor<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        if let StmtKind::Const(items) = &stmt.kind {
            for item in items.iter() {
                if item.name == self.const_name {
                    let name = item.name.to_string();
                    if let Some(start) = str_offset_in_range(self.source, item.span, &name) {
                        self.out.push(Span {
                            start,
                            end: start + name.len() as u32,
                        });
                    }
                }
                // Visit the value expression for any constant references inside it.
                let _ = self.visit_expr(&item.value);
            }
            return ControlFlow::Continue(());
        }
        // Handle `define('NAME', value)` declaration at statement level.
        if let StmtKind::Expression(expr) = &stmt.kind
            && let ExprKind::FunctionCall(f) = &expr.kind
            && let ExprKind::Identifier(id) = &f.name.kind
            && id.as_str() == "define"
            && let Some(first_arg) = f.args.first()
            && let ExprKind::String(s) = &first_arg.value.kind
            && *s == self.const_name
        {
            // Span covers the string content excluding the surrounding quotes.
            let start = first_arg.value.span.start + 1;
            self.out.push(Span {
                start,
                end: start + s.len() as u32,
            });
            // Don't recurse: the second arg (value) is not a constant reference.
            return ControlFlow::Continue(());
        }
        walk_stmt(self, stmt)
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            ExprKind::Identifier(name) => {
                let s = name.as_str();
                // Bare name in same namespace.
                let name_offset = if s == self.const_name {
                    Some(0usize)
                } else if let Some(fqn) = self.const_fqn {
                    // Qualified: `Config\DB_HOST` or `\Config\DB_HOST`.
                    let bare_fqn = s.trim_start_matches('\\');
                    if bare_fqn == fqn {
                        // Offset of const_name within the identifier string.
                        Some(s.len() - self.const_name.len())
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(off) = name_offset {
                    let start = expr.span.start + off as u32;
                    self.out.push(Span {
                        start,
                        end: start + self.const_name.len() as u32,
                    });
                }
                ControlFlow::Continue(())
            }

            // Function call: skip the callee name, only visit arguments.
            ExprKind::FunctionCall(f) => {
                for arg in f.args.iter() {
                    let _ = self.visit_arg(arg);
                }
                ControlFlow::Continue(())
            }

            // Static method call: skip class and method names, only visit arguments.
            ExprKind::StaticMethodCall(call) => {
                for arg in call.args.iter() {
                    let _ = self.visit_arg(arg);
                }
                ControlFlow::Continue(())
            }
            ExprKind::StaticDynMethodCall(call) => {
                for arg in call.args.iter() {
                    let _ = self.visit_arg(arg);
                }
                ControlFlow::Continue(())
            }

            // New expression: skip the class name, only visit constructor arguments.
            ExprKind::New(new_expr) => {
                for arg in new_expr.args.iter() {
                    let _ = self.visit_arg(arg);
                }
                ControlFlow::Continue(())
            }

            // Static property / class-const access: skip entirely (class names and
            // class-const member names are not global constant references).
            ExprKind::StaticPropertyAccess(_)
            | ExprKind::ClassConstAccess(_)
            | ExprKind::ClassConstAccessDynamic { .. }
            | ExprKind::StaticPropertyAccessDynamic { .. } => ControlFlow::Continue(()),

            // instanceof right-hand side is a class name, not a constant.
            ExprKind::Binary(b) if b.op == php_ast::BinaryOp::Instanceof => {
                let _ = self.visit_expr(b.left);
                // Skip b.right — it's a class name identifier, not a constant.
                ControlFlow::Continue(())
            }

            _ => walk_expr(self, expr),
        }
    }
}

// ── Class-reference walker ────────────────────────────────────────────────────

/// Collect spans for `new ClassName(...)` expressions only — excludes type hints,
/// `instanceof`, `extends`, `implements`, and static calls.
///
/// `class_fqn` — when `Some`, FQN-qualified identifiers in the source (those
/// containing `\`) are compared against the FQN rather than just the short name,
/// preventing false positives when two classes share a short name across namespaces.
pub fn new_refs_in_stmts(
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    class_fqn: Option<&str>,
    out: &mut Vec<Span>,
) {
    let mut v = NewRefsVisitor {
        class_name,
        class_fqn,
        out: Vec::new(),
    };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    out.append(&mut v.out);
}

struct NewRefsVisitor<'a> {
    class_name: &'a str,
    class_fqn: Option<&'a str>,
    out: Vec<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for NewRefsVisitor<'_> {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::New(n) = &expr.kind
            && let ExprKind::Identifier(id) = &n.class.kind
        {
            let matches = if id.contains('\\')
                && let Some(fqn) = self.class_fqn
            {
                // Fully-qualified identifier: compare by FQN for exact namespace match.
                id.trim_start_matches('\\') == fqn.trim_start_matches('\\')
            } else {
                fqn_short_name(id) == self.class_name
            };
            if matches {
                self.out.push(n.class.span);
            }
        }
        walk_expr(self, expr)
    }
}

/// Collect every fully-qualified class name (i.e. starting with `\`) that
/// appears as the class argument of a `new` expression in `stmts`.
/// Returns de-duplicated FQCN strings with the leading `\` stripped, ready to
/// pass to `session.lazy_load_class`.
pub fn fqn_new_class_refs_in_stmts(stmts: &[Stmt<'_, '_>]) -> Vec<String> {
    let mut v = FqnNewRefsVisitor { out: Vec::new() };
    for stmt in stmts {
        let _ = v.visit_stmt(stmt);
    }
    v.out.sort_unstable();
    v.out.dedup();
    v.out
}

struct FqnNewRefsVisitor {
    out: Vec<String>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for FqnNewRefsVisitor {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::New(n) = &expr.kind
            && let ExprKind::Identifier(id) = &n.class.kind
            && id.starts_with('\\')
        {
            self.out.push(id.trim_start_matches('\\').to_string());
        }
        walk_expr(self, expr)
    }
}

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
        let last = fqn_short_name(&repr);
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
                    && fqn_short_name(id) == self.class_name
                {
                    self.out.push(n.class.span);
                }
            }
            ExprKind::AnonymousClass(c) => {
                if let Some(ext) = &c.extends {
                    self.collect_name(ext);
                }
                for iface in c.implements.iter() {
                    self.collect_name(iface);
                }
            }
            ExprKind::Binary(b) => {
                if let ExprKind::Identifier(id) = &b.right.kind
                    && fqn_short_name(id) == self.class_name
                {
                    self.out.push(b.right.span);
                }
            }
            ExprKind::StaticMethodCall(s) => {
                if let ExprKind::Identifier(id) = &s.class.kind
                    && fqn_short_name(id) == self.class_name
                {
                    self.out.push(s.class.span);
                }
            }
            ExprKind::StaticPropertyAccess(s) => {
                if let ExprKind::Identifier(id) = &s.class.kind
                    && fqn_short_name(id) == self.class_name
                {
                    self.out.push(s.class.span);
                }
            }
            ExprKind::ClassConstAccess(c) => {
                if let ExprKind::Identifier(id) = &c.class.kind
                    && fqn_short_name(id) == self.class_name
                {
                    self.out.push(c.class.span);
                }
            }
            _ => {}
        }
        walk_expr(self, expr)
    }

    fn visit_attribute(&mut self, attribute: &Attribute<'arena, 'src>) -> ControlFlow<()> {
        self.collect_name(&attribute.name);
        walk_attribute(self, attribute)
    }

    fn visit_type_hint(&mut self, type_hint: &TypeHint<'arena, 'src>) -> ControlFlow<()> {
        match &type_hint.kind {
            TypeHintKind::Named(name) => {
                self.collect_name(name);
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

    #[test]
    fn property_refs_finds_static_access() {
        let src = "<?php\nclass Reg { public static int $val = 0; }\nReg::$val;\nReg::$val = 1;";
        let doc = parse(src);
        let mut out = vec![];
        property_refs_in_stmts(src, &doc.program().stmts, "val", &mut out);
        // declaration + two static access sites
        assert_eq!(out.len(), 3, "expected decl + 2 accesses, got: {out:?}");
    }

    // ── constant_refs_in_stmts ──────────────────────────────────────────────

    #[test]
    fn constant_refs_finds_decl_and_class_access() {
        let src = "<?php\nclass S { const ACTIVE = 1; }\n$x = S::ACTIVE;\nif ($v === S::ACTIVE) {}";
        let doc = parse(src);
        let mut out = vec![];
        constant_refs_in_stmts(src, &doc.program().stmts, "ACTIVE", None, &mut out);
        // declaration + 2 access sites
        assert_eq!(out.len(), 3, "expected decl + 2 accesses, got: {out:?}");
    }

    #[test]
    fn constant_refs_finds_self_and_parent_access() {
        let src = "<?php\nclass Base { const V = 1; }\nclass Child extends Base { public function f(): int { return parent::V; } }";
        let doc = parse(src);
        let mut out = vec![];
        constant_refs_in_stmts(src, &doc.program().stmts, "V", Some("Base"), &mut out);
        let texts = spans_to_strs(src, &out);
        // should find: declaration in Base + parent::V access in Child
        assert!(
            out.len() >= 2,
            "expected decl + parent::V access, got: {texts:?}"
        );
    }

    #[test]
    fn constant_refs_parent_reference_full_source() {
        let src = "<?php\nclass Base {\n    const VERSION = '1.0';\n}\n\nclass Extended extends Base {\n    public function getVersion(): string {\n        return parent::VERSION;\n    }\n}\n\necho Extended::VERSION;";
        let doc = parse(src);
        let mut out = vec![];
        constant_refs_in_stmts(src, &doc.program().stmts, "VERSION", Some("Base"), &mut out);
        let texts = spans_to_strs(src, &out);
        assert!(
            out.len() >= 3,
            "expected decl + parent::VERSION + Extended::VERSION = 3, got {}: {texts:?}",
            out.len()
        );
    }

    #[test]
    fn constant_refs_filters_same_name_different_class() {
        let src = "<?php\nclass A { const X = 1; }\nclass B { const X = 2; }\nA::X;\nB::X;";
        let doc = parse(src);
        let mut out = vec![];
        constant_refs_in_stmts(src, &doc.program().stmts, "X", Some("A"), &mut out);
        // should NOT include B's declaration or B::X
        let texts = spans_to_strs(src, &out);
        assert!(!texts.is_empty(), "should find A::X: {texts:?}");
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

    // ── all_class_ref_names_in_stmts ─────────────────────────────────────────

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
