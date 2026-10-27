/// Code action: "Inline variable" — replaces all usages of a variable with its
/// initializer expression and removes the assignment statement.
///
/// Only acts when:
/// - The cursor/selection is on or inside a variable name (e.g. `$extracted`).
/// - The variable is bound exactly once in its function scope, by a plain
///   `$var = <expr>;` statement that precedes every read and sits in the same
///   block as all of them.
/// - Every read is a plain value: not interpolated, captured by a closure,
///   checked by `isset`, assigned to, bound by reference or iterated by
///   reference; nothing in the scope reads variables by name (`compact`, `$$`).
/// - The initializer is safe to evaluate at each read:
///   - a constant expression, anywhere;
///   - variables and property/array reads, if nothing but writes to other
///     locals and output runs between the assignment and the last read;
///   - anything else, read once, unconditionally, in the next statement, with
///     nothing observable evaluated before the read.
use std::collections::HashMap;
use std::ops::ControlFlow;

use php_ast::{
    Arg, AssignOp, BinaryOp, Block, ClassMember, ClassMemberKind, Expr, ExprKind, Span, Stmt,
    StmtKind, UnaryPrefixOp,
    visitor::{Visitor, walk_block, walk_class_member, walk_expr, walk_stmt},
};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, DocumentHighlightKind, Range, TextEdit, Uri,
    WorkspaceEdit,
};

use crate::document::ast::ParsedDoc;
use crate::navigation::walk::collect_var_refs_in_scope;
use crate::text::word_at_position;

pub fn inline_variable_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
    is_byref_param: &dyn Fn(ArgSlot<'_>) -> bool,
) -> Vec<CodeActionOrCommand> {
    let var_name = match word_at_position(source, range.start) {
        Some(w) if w.starts_with('$') && w.len() > 1 => w,
        _ => return vec![],
    };
    let name = &var_name[1..];
    let sv = doc.view();
    let stmts = &doc.program().stmts;

    let mut refs = Vec::new();
    collect_var_refs_in_scope(
        stmts,
        name,
        sv.byte_of_position(range.start) as usize,
        &mut refs,
    );
    refs.sort_by_key(|(span, _)| span.start);
    refs.dedup_by_key(|(span, _)| span.start);

    let mut writes = refs
        .iter()
        .filter(|(_, kind)| *kind == DocumentHighlightKind::WRITE);
    let (Some(&(write_span, _)), None) = (writes.next(), writes.next()) else {
        return vec![];
    };
    let reads: Vec<Span> = refs
        .iter()
        .filter(|(_, kind)| *kind == DocumentHighlightKind::READ)
        .map(|(span, _)| *span)
        .collect();

    let mut finder = AssignmentFinder {
        target: write_span,
        containers: Vec::new(),
        scopes: Vec::new(),
        found: None,
    };
    for stmt in stmts.iter() {
        let _ = finder.visit_stmt(stmt);
    }
    let Some(found) = finder.found else {
        return vec![];
    };
    let Some(region) = found.container else {
        return vec![];
    };
    let region = region.unwrap_or(Span::new(0, source.len() as u32));

    let mut scan = ScopeScan::new(
        source,
        found.scope.unwrap_or(Span::new(0, source.len() as u32)),
    );
    for stmt in stmts.iter() {
        let _ = scan.visit_stmt(stmt);
    }
    if scan.dynamic_scope
        || scan
            .bindings
            .iter()
            .any(|(span, bound)| bound == name && *span != write_span)
    {
        return vec![];
    }
    let usable = |read: &Span| {
        read.start >= found.stmt.end
            && contains(region, *read)
            && !scan.zones.iter().any(|zone| contains(*zone, *read))
    };
    if reads.is_empty() || !reads.iter().all(usable) {
        return vec![];
    }
    // Loops, closures and branches that run after the assignment, apart from it.
    let after = |spans: &[Span], read: &Span| {
        spans
            .iter()
            .filter(|span| span.start >= found.stmt.end && contains(**span, *read))
            .map(|span| span.end)
            .max()
    };
    let safe_to_evaluate_at_reads = match found.initializer {
        Initializer::Constant => true,
        Initializer::Pure => {
            let cutoff = reads
                .iter()
                .map(|read| after(&scan.loops, read).unwrap_or(read.start))
                .max()
                .unwrap_or(found.stmt.end);
            !reads
                .iter()
                .any(|read| after(&scan.arrow_fns, read).is_some())
                && !found
                    .value_vars
                    .iter()
                    .any(|var| scan.ref_bound.contains(var))
                && !scan.effects.iter().any(|(span, effect)| {
                    span.start >= found.stmt.end
                        && span.end <= cutoff
                        && match effect {
                            Effect::Output => false,
                            Effect::Write(var) => found.value_vars.contains(var),
                            Effect::Other => true,
                        }
                })
        }
        Initializer::Effectful => {
            let [read] = reads.as_slice() else {
                return vec![];
            };
            let between = |span: &Span| span.start >= found.stmt.end && span.end <= read.start;
            [&scan.loops, &scan.arrow_fns, &scan.conditionals]
                .iter()
                .all(|spans| after(spans, read).is_none())
                && !scan.stmts.iter().any(between)
                && !scan.effects.iter().any(|(span, _)| between(span))
        }
    };
    if !safe_to_evaluate_at_reads {
        return vec![];
    }

    let mut sites = ReadSites {
        source,
        reads: &reads,
        args: Vec::new(),
        callees: Vec::new(),
        class_refs: Vec::new(),
    };
    for stmt in stmts.iter() {
        let _ = sites.visit_stmt(stmt);
    }
    let callees = sites.callees;
    let class_refs = sites.class_refs;
    let class_name = found
        .class_name
        .map(|span| &source[span.start as usize..span.end as usize]);
    if !class_refs.is_empty() && class_name.is_none() {
        return vec![];
    }
    if sites.args.into_iter().any(is_byref_param) {
        return vec![];
    }

    let rhs = &source[found.value.start as usize..found.value.end as usize];
    let mut edits: Vec<TextEdit> = reads
        .iter()
        .map(|read| {
            let new_text = if class_refs.contains(read)
                && let Some(class_name) = class_name
            {
                class_name.to_string()
            } else if callees.contains(read) {
                if found.value_is_variable {
                    rhs.to_string()
                } else {
                    format!("({rhs})")
                }
            } else if found.value_is_number && touches_dot(source, *read) {
                // `1.'a'` and `'a'.1` lex as float literals.
                format!("({rhs})")
            } else if found.value_is_atomic || stands_alone(source, *read) {
                rhs.to_string()
            } else {
                format!("({rhs})")
            };
            TextEdit {
                range: Range {
                    start: sv.position_of(read.start),
                    end: sv.position_of(read.end),
                },
                new_text,
            }
        })
        .collect();

    let (del_start, del_end) = statement_deletion(source, found.stmt);
    edits.push(TextEdit {
        range: Range {
            start: sv.position_of(del_start),
            end: sv.position_of(del_end),
        },
        new_text: String::new(),
    });

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Inline variable '{var_name}'"),
        kind: Some(CodeActionKind::REFACTOR_INLINE),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

fn contains(outer: Span, inner: Span) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

fn touches_dot(source: &str, span: Span) -> bool {
    source[..span.start as usize].ends_with('.') || source[span.end as usize..].starts_with('.')
}

/// How freely the initializer can be evaluated at the reads instead.
enum Initializer {
    /// Same value wherever and however often it's evaluated.
    Constant,
    /// No side effect, but the value depends on variables or object state.
    Pure,
    /// Calls, `new`, assignments: must run exactly once, where it ran before.
    Effectful,
}

struct FoundAssignment {
    stmt: Span,
    value: Span,
    initializer: Initializer,
    value_is_atomic: bool,
    value_is_variable: bool,
    value_is_number: bool,
    /// What replaces a read in a class position (`new`, `instanceof`, `::`):
    /// the variable, or `Foo` of `Foo::class`.
    class_name: Option<Span>,
    /// Variables the initializer reads, other than `$this`.
    value_vars: Vec<String>,
    /// The innermost function, method or closure; `None` at the top level.
    scope: Option<Span>,
    /// `Some(None)`: a direct child of the scope's top-level statement list;
    /// `Some(Some(block))`: a direct child of `block`; `None`: the body of a
    /// brace-less `if`/loop/`switch` case, which runs conditionally.
    container: Option<Option<Span>>,
}

/// Finds the `$var = <expr>;` statement whose target is `target` and the
/// statement list it belongs to.
struct AssignmentFinder {
    target: Span,
    /// Innermost-last: `Some(block)` for a block, `None` for a statement whose
    /// body isn't a statement list.
    containers: Vec<Option<Span>>,
    /// Innermost-last enclosing functions, methods and closures.
    scopes: Vec<Span>,
    found: Option<FoundAssignment>,
}

impl AssignmentFinder {
    fn in_scope(
        &mut self,
        span: Span,
        walk: impl FnOnce(&mut Self) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        let containers = std::mem::take(&mut self.containers);
        self.scopes.push(span);
        let flow = walk(self);
        self.scopes.pop();
        self.containers = containers;
        flow
    }
}

impl<'arena, 'src> Visitor<'arena, 'src> for AssignmentFinder {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        if let StmtKind::Expression(expr) = &stmt.kind
            && let ExprKind::Assign(assign) = &expr.kind
            && assign.target.span == self.target
        {
            if assign.op == AssignOp::Assign && !assign.by_ref {
                let value = assign.value;
                self.found = Some(FoundAssignment {
                    stmt: stmt.span,
                    value: value.span,
                    initializer: if !is_repeatable(value) {
                        Initializer::Effectful
                    } else if reads_state(value) {
                        Initializer::Pure
                    } else {
                        Initializer::Constant
                    },
                    value_is_atomic: is_atomic(value),
                    value_is_variable: matches!(
                        value.kind,
                        ExprKind::Variable(_) | ExprKind::Parenthesized(_)
                    ),
                    value_is_number: matches!(value.kind, ExprKind::Int(_) | ExprKind::Float(_)),
                    class_name: class_name(value),
                    value_vars: variable_names(value),
                    scope: self.scopes.last().copied(),
                    container: match self.containers.last() {
                        None => Some(None),
                        Some(Some(block)) => Some(Some(*block)),
                        Some(None) => None,
                    },
                });
            }
            return ControlFlow::Break(());
        }
        if let StmtKind::Function(_) = stmt.kind {
            return self.in_scope(stmt.span, |this| walk_stmt(this, stmt));
        }
        let conditional_body = matches!(
            stmt.kind,
            StmtKind::If(_)
                | StmtKind::While(_)
                | StmtKind::For(_)
                | StmtKind::Foreach(_)
                | StmtKind::DoWhile(_)
                | StmtKind::Switch(_)
                | StmtKind::Declare(_)
        );
        if conditional_body {
            self.containers.push(None);
        }
        let flow = walk_stmt(self, stmt);
        if conditional_body {
            self.containers.pop();
        }
        flow
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::Closure(_) = expr.kind {
            return self.in_scope(expr.span, |this| walk_expr(this, expr));
        }
        walk_expr(self, expr)
    }

    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        if let ClassMemberKind::Method(_) = member.kind {
            return self.in_scope(member.span, |this| walk_class_member(this, member));
        }
        walk_class_member(self, member)
    }

    fn visit_block(&mut self, block: &Block<'arena, 'src>) -> ControlFlow<()> {
        self.containers.push(Some(block.span));
        let flow = walk_block(self, block);
        self.containers.pop();
        flow
    }
}

/// Whether evaluating `expr` twice gives the same value with no side effect,
/// so it can replace more than one read. Calls and `new` don't qualify.
fn is_repeatable(expr: &Expr<'_, '_>) -> bool {
    match &expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Nowdoc { .. }
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::Variable(_)
        | ExprKind::Identifier(_)
        | ExprKind::MagicConst(_)
        | ExprKind::ClassConstAccess(_) => true,
        ExprKind::Parenthesized(inner) | ExprKind::Cast(_, inner) => is_repeatable(inner),
        ExprKind::UnaryPrefix(unary) => {
            !matches!(
                unary.op,
                UnaryPrefixOp::PreIncrement | UnaryPrefixOp::PreDecrement
            ) && is_repeatable(unary.operand)
        }
        ExprKind::Binary(binary) => {
            binary.op != BinaryOp::Pipe && is_repeatable(binary.left) && is_repeatable(binary.right)
        }
        ExprKind::PropertyAccess(access) | ExprKind::NullsafePropertyAccess(access) => {
            is_repeatable(access.object) && is_repeatable(access.property)
        }
        ExprKind::StaticPropertyAccess(access) => is_repeatable(access.class),
        ExprKind::ArrayAccess(access) => {
            is_repeatable(access.array) && access.index.is_none_or(is_repeatable)
        }
        ExprKind::Array(elements) => elements.iter().all(|element| {
            !element.by_ref
                && element.key.as_ref().is_none_or(is_repeatable)
                && is_repeatable(&element.value)
        }),
        _ => false,
    }
}

fn class_name(expr: &Expr<'_, '_>) -> Option<Span> {
    match &expr.kind {
        ExprKind::Variable(_) => Some(expr.span),
        ExprKind::ClassConstAccess(access)
            if matches!(access.class.kind, ExprKind::Identifier(_))
                && access
                    .member
                    .name_str()
                    .is_some_and(|m| m.eq_ignore_ascii_case("class")) =>
        {
            Some(access.class.span)
        }
        _ => None,
    }
}

fn variable_names(expr: &Expr<'_, '_>) -> Vec<String> {
    struct Names(Vec<String>);
    impl<'arena, 'src> Visitor<'arena, 'src> for Names {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if let ExprKind::Variable(name) = &expr.kind
                && name.as_str() != "this"
                && !self.0.iter().any(|n| n == name.as_str())
            {
                self.0.push(name.as_str().to_string());
            }
            walk_expr(self, expr)
        }
    }
    let mut names = Names(Vec::new());
    let _ = names.visit_expr(expr);
    names.0
}

/// Whether `expr` reads a variable or a static property.
fn reads_state(expr: &Expr<'_, '_>) -> bool {
    struct State(bool);
    impl<'arena, 'src> Visitor<'arena, 'src> for State {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if matches!(
                expr.kind,
                ExprKind::Variable(_)
                    | ExprKind::StaticPropertyAccess(_)
                    | ExprKind::StaticPropertyAccessDynamic { .. }
            ) {
                self.0 = true;
                return ControlFlow::Break(());
            }
            walk_expr(self, expr)
        }
    }
    let mut state = State(false);
    let _ = state.visit_expr(expr);
    state.0
}

/// A read passed as a whole argument: the callee's name starts at
/// `callee_offset`, and the argument is the `position`-th or named `name`.
pub struct ArgSlot<'a> {
    pub callee_offset: u32,
    pub position: usize,
    pub name: Option<&'a str>,
}

/// Finds where the reads sit in their enclosing expressions.
struct ReadSites<'s> {
    source: &'s str,
    reads: &'s [Span],
    args: Vec<ArgSlot<'s>>,
    /// Reads called as a function: `$read(...)`.
    callees: Vec<Span>,
    /// Reads naming a class: `new $read`, `$x instanceof $read`, `$read::f()`.
    class_refs: Vec<Span>,
}

impl ReadSites<'_> {
    fn note_class_ref(&mut self, span: Span) {
        if self.reads.contains(&span) {
            self.class_refs.push(span);
        }
    }

    fn note_args(&mut self, callee_offset: u32, args: &[Arg<'_, '_>]) {
        for (position, arg) in args.iter().enumerate() {
            if let Some(value) = &arg.value
                && !arg.unpack
                && self.reads.contains(&value.span)
            {
                let source = self.source;
                self.args.push(ArgSlot {
                    callee_offset,
                    position,
                    name: arg.name.as_ref().map(|n| {
                        let span = n.span();
                        &source[span.start as usize..span.end as usize]
                    }),
                });
            }
        }
    }
}

impl<'arena, 'src> Visitor<'arena, 'src> for ReadSites<'_> {
    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            ExprKind::FunctionCall(call) => {
                if self.reads.contains(&call.name.span) {
                    self.callees.push(call.name.span);
                }
                self.note_args(call.name.span.start, &call.args)
            }
            ExprKind::MethodCall(call) | ExprKind::NullsafeMethodCall(call) => {
                self.note_args(call.method.span.start, &call.args)
            }
            ExprKind::StaticMethodCall(call) => {
                self.note_class_ref(call.class.span);
                self.note_args(call.method.span.start, &call.args)
            }
            ExprKind::StaticPropertyAccess(access) | ExprKind::ClassConstAccess(access) => {
                self.note_class_ref(access.class.span)
            }
            ExprKind::New(new) => {
                self.note_class_ref(new.class.span);
                self.note_args(new.class.span.start, &new.args)
            }
            ExprKind::Binary(binary) if binary.op == BinaryOp::Instanceof => {
                self.note_class_ref(binary.right.span)
            }
            _ => {}
        }
        walk_expr(self, expr)
    }
}

/// Something evaluated in the scope that the initializer could observe.
enum Effect {
    /// Assigns a local: `$name = …`, `$name++`, `foreach (… as $name)`.
    Write(String),
    /// `echo`, `print`.
    Output,
    /// A call, `new`, a write to a property, element or reference, …
    Other,
}

/// One pass over the inlined variable's scope, skipping nested functions.
struct ScopeScan<'s> {
    source: &'s str,
    scope: Span,
    /// Spans where a variable isn't a plain read.
    zones: Vec<Span>,
    /// Variables assigned, iterated into, unset, caught, or declared `global`/`static`.
    bindings: Vec<(Span, String)>,
    /// Variables bound by reference: `&$x`, `global $x`, `static $x`.
    ref_bound: Vec<String>,
    effects: Vec<(Span, Effect)>,
    stmts: Vec<Span>,
    /// The repeated part of each loop: everything but `for` init and `foreach` subject.
    loops: Vec<Span>,
    arrow_fns: Vec<Span>,
    /// Branches that may not run: `if` bodies, `?:` arms, `&&` right operands, …
    conditionals: Vec<Span>,
    /// `compact`, `extract`, `$$x`, `eval`, `include`: variables used by name.
    dynamic_scope: bool,
}

impl<'s> ScopeScan<'s> {
    fn new(source: &'s str, scope: Span) -> Self {
        Self {
            source,
            scope,
            zones: Vec::new(),
            bindings: Vec::new(),
            ref_bound: Vec::new(),
            effects: Vec::new(),
            stmts: Vec::new(),
            loops: Vec::new(),
            arrow_fns: Vec::new(),
            conditionals: Vec::new(),
            dynamic_scope: false,
        }
    }

    /// Records the variables an assignment-like `target` rebinds or mutates.
    fn bind(&mut self, target: &Expr<'_, '_>, by_ref: bool) {
        match &target.kind {
            ExprKind::Variable(name) => {
                self.bindings.push((target.span, name.to_string()));
                if by_ref {
                    self.ref_bound.push(name.to_string());
                }
            }
            ExprKind::Array(elements) => {
                for element in elements.iter() {
                    self.bind(&element.value, element.by_ref);
                }
            }
            ExprKind::ArrayAccess(access) => self.bind(access.array, by_ref),
            ExprKind::PropertyAccess(access) | ExprKind::NullsafePropertyAccess(access) => {
                self.bind(access.object, false)
            }
            ExprKind::Parenthesized(inner) => self.bind(inner, by_ref),
            _ => {}
        }
    }

    fn write(&mut self, span: Span, target: &Expr<'_, '_>) {
        let effect = match &target.kind {
            ExprKind::Variable(name) => Effect::Write(name.to_string()),
            _ => Effect::Other,
        };
        self.effects.push((span, effect));
    }
}

fn is_dynamic_scope_call(name: &Expr<'_, '_>) -> bool {
    name.name_str().is_some_and(|name| {
        let name = name.trim_start_matches('\\');
        ["compact", "extract", "get_defined_vars", "parse_str"]
            .iter()
            .any(|f| name.eq_ignore_ascii_case(f))
    })
}

impl<'arena, 'src> Visitor<'arena, 'src> for ScopeScan<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        let scope_like = matches!(
            stmt.kind,
            StmtKind::Function(_)
                | StmtKind::Class(_)
                | StmtKind::Interface(_)
                | StmtKind::Trait(_)
                | StmtKind::Enum(_)
        );
        if !contains(self.scope, stmt.span) || (scope_like && stmt.span != self.scope) {
            return if contains(stmt.span, self.scope) {
                walk_stmt(self, stmt)
            } else {
                ControlFlow::Continue(())
            };
        }
        self.stmts.push(stmt.span);
        match &stmt.kind {
            StmtKind::Echo(_) => self.effects.push((stmt.span, Effect::Output)),
            StmtKind::Unset(targets) => {
                self.zones.push(stmt.span);
                self.effects.push((stmt.span, Effect::Other));
                for target in targets.iter() {
                    self.bind(target, false);
                }
            }
            StmtKind::Global(vars) => {
                self.zones.push(stmt.span);
                self.effects.push((stmt.span, Effect::Other));
                for var in vars.iter() {
                    self.bind(var, true);
                }
            }
            StmtKind::StaticVar(vars) => {
                self.zones.push(stmt.span);
                self.effects.push((stmt.span, Effect::Other));
                for var in vars.iter() {
                    let name = var.name.or_error().to_string();
                    self.bindings.push((var.span, name.clone()));
                    self.ref_bound.push(name);
                }
            }
            StmtKind::Foreach(foreach) => {
                let by_ref = self.source[..foreach.value.span.start as usize]
                    .trim_end()
                    .ends_with('&');
                let first = foreach.key.as_ref().unwrap_or(&foreach.value);
                self.loops.push(Span::new(first.span.start, stmt.span.end));
                if by_ref {
                    self.zones.push(foreach.expr.span);
                    self.bind(&foreach.expr, true);
                }
                for target in foreach.key.iter().chain([&foreach.value]) {
                    self.zones.push(target.span);
                    self.write(target.span, target);
                }
                if let Some(key) = &foreach.key {
                    self.bind(key, false);
                }
                self.bind(&foreach.value, by_ref);
            }
            StmtKind::For(for_stmt) => {
                let first = for_stmt
                    .condition
                    .first()
                    .or(for_stmt.update.first())
                    .map_or(for_stmt.body.span.start, |expr| expr.span.start);
                self.loops.push(Span::new(first, stmt.span.end));
            }
            StmtKind::While(_) | StmtKind::DoWhile(_) => self.loops.push(stmt.span),
            StmtKind::If(if_stmt) => {
                self.conditionals.push(if_stmt.then_branch.span);
                self.conditionals
                    .extend(if_stmt.elseif_branches.iter().map(|branch| branch.span));
                self.conditionals
                    .extend(if_stmt.else_branch.map(|branch| branch.span));
            }
            StmtKind::Switch(switch) => self.conditionals.push(switch.body.span),
            StmtKind::TryCatch(try_catch) => {
                for catch in try_catch.catches.iter() {
                    self.conditionals.push(catch.span);
                    if let Some(var) = catch.var {
                        let span = Span::new(catch.span.start, catch.body.span.start);
                        self.bindings.push((span, var.to_string()));
                        self.effects.push((span, Effect::Write(var.to_string())));
                    }
                }
            }
            _ => {}
        }
        walk_stmt(self, stmt)
    }

    fn visit_class_member(&mut self, member: &ClassMember<'arena, 'src>) -> ControlFlow<()> {
        if contains(member.span, self.scope) {
            walk_class_member(self, member)
        } else {
            ControlFlow::Continue(())
        }
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        if !contains(self.scope, expr.span) {
            return if contains(expr.span, self.scope) {
                walk_expr(self, expr)
            } else {
                ControlFlow::Continue(())
            };
        }
        match &expr.kind {
            ExprKind::Closure(closure) if expr.span != self.scope => {
                for use_var in closure.use_vars.iter() {
                    self.zones.push(use_var.span);
                    if use_var.by_ref {
                        let name = use_var.name.trim_start_matches('$').to_string();
                        self.bindings.push((use_var.span, name.clone()));
                        self.ref_bound.push(name);
                    }
                }
                return ControlFlow::Continue(());
            }
            ExprKind::AnonymousClass(_) => {
                self.effects.push((expr.span, Effect::Other));
                return ControlFlow::Continue(());
            }
            ExprKind::InterpolatedString(_) | ExprKind::Heredoc { .. } | ExprKind::Isset(_) => {
                self.zones.push(expr.span)
            }
            ExprKind::ShellExec(_) => {
                self.zones.push(expr.span);
                self.effects.push((expr.span, Effect::Other));
            }
            ExprKind::Assign(assign) => {
                self.zones.push(assign.target.span);
                self.bind(assign.target, assign.by_ref);
                if assign.by_ref {
                    self.zones.push(assign.value.span);
                    self.bind(assign.value, true);
                    self.effects.push((expr.span, Effect::Other));
                } else {
                    self.write(expr.span, assign.target);
                }
            }
            ExprKind::UnaryPrefix(unary)
                if matches!(
                    unary.op,
                    UnaryPrefixOp::PreIncrement | UnaryPrefixOp::PreDecrement
                ) =>
            {
                self.zones.push(unary.operand.span);
                self.bind(unary.operand, false);
                self.write(expr.span, unary.operand);
            }
            ExprKind::UnaryPostfix(unary) => {
                self.zones.push(unary.operand.span);
                self.bind(unary.operand, false);
                self.write(expr.span, unary.operand);
            }
            ExprKind::Array(elements) => {
                for element in elements.iter().filter(|element| element.by_ref) {
                    self.zones.push(element.value.span);
                    self.bind(&element.value, true);
                }
            }
            ExprKind::FunctionCall(call) => {
                self.dynamic_scope |= is_dynamic_scope_call(call.name);
                self.effects.push((expr.span, Effect::Other));
            }
            ExprKind::NullsafeMethodCall(call) => {
                self.conditionals
                    .push(Span::new(call.method.span.start, expr.span.end));
                self.effects.push((expr.span, Effect::Other));
            }
            ExprKind::MethodCall(_)
            | ExprKind::StaticMethodCall(_)
            | ExprKind::StaticDynMethodCall(_)
            | ExprKind::New(_)
            | ExprKind::Clone(_)
            | ExprKind::CloneWith(..)
            | ExprKind::Exit(_)
            | ExprKind::Yield(_)
            | ExprKind::ThrowExpr(_) => self.effects.push((expr.span, Effect::Other)),
            ExprKind::Include(..) | ExprKind::Eval(_) | ExprKind::VariableVariable(_) => {
                self.dynamic_scope = true;
            }
            ExprKind::Print(_) => self.effects.push((expr.span, Effect::Output)),
            ExprKind::Ternary(ternary) => {
                self.conditionals
                    .extend(ternary.then_expr.map(|then| then.span));
                self.conditionals.push(ternary.else_expr.span);
            }
            ExprKind::NullCoalesce(coalesce) => self.conditionals.push(coalesce.right.span),
            ExprKind::Binary(binary)
                if matches!(
                    binary.op,
                    BinaryOp::BooleanAnd
                        | BinaryOp::BooleanOr
                        | BinaryOp::LogicalAnd
                        | BinaryOp::LogicalOr
                ) =>
            {
                self.conditionals.push(binary.right.span)
            }
            ExprKind::Match(match_expr) => self
                .conditionals
                .extend(match_expr.arms.iter().map(|arm| arm.span)),
            ExprKind::ArrowFunction(_) => self.arrow_fns.push(expr.span),
            _ => {}
        }
        walk_expr(self, expr)
    }
}

/// Whether `expr` binds tighter than any operator it could be placed next to.
fn is_atomic(expr: &Expr<'_, '_>) -> bool {
    matches!(
        expr.kind,
        ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::String(_)
            | ExprKind::InterpolatedString(_)
            | ExprKind::Heredoc { .. }
            | ExprKind::Nowdoc { .. }
            | ExprKind::Bool(_)
            | ExprKind::Null
            | ExprKind::Variable(_)
            | ExprKind::Identifier(_)
            | ExprKind::FunctionCall(_)
            | ExprKind::Array(_)
            | ExprKind::ArrayAccess(_)
            | ExprKind::Parenthesized(_)
            | ExprKind::MagicConst(_)
            | ExprKind::PropertyAccess(_)
            | ExprKind::NullsafePropertyAccess(_)
            | ExprKind::MethodCall(_)
            | ExprKind::NullsafeMethodCall(_)
            | ExprKind::StaticPropertyAccess(_)
            | ExprKind::StaticMethodCall(_)
            | ExprKind::ClassConstAccess(_)
    )
}

/// Whether the read at `span` is a whole expression on its own — an argument,
/// array element, assigned value, or `return`/`echo` operand — so it needs no
/// parentheses whatever replaces it.
fn stands_alone(source: &str, span: Span) -> bool {
    let before = source[..span.start as usize].trim_end();
    let after = source[span.end as usize..].trim_start();
    let opens = before.ends_with(['(', '[', ','])
        || before.ends_with("=>")
        || (before.ends_with('=')
            && !["==", "!=", "<=", ">="]
                .iter()
                .any(|op| before.ends_with(op)))
        || ["return", "echo", "print"].iter().any(|kw| {
            before.strip_suffix(kw).is_some_and(|rest| {
                !rest.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
            })
        });
    opens && after.starts_with([';', ')', ']', ','])
}

/// Byte range to delete for the statement at `stmt`: its whole line(s) when
/// nothing else shares them, otherwise the statement and the space after it.
fn statement_deletion(source: &str, stmt: Span) -> (u32, u32) {
    let (start, end) = (stmt.start as usize, stmt.end as usize);
    let line_start = source[..start].rfind('\n').map_or(0, |i| i + 1);
    let rest_of_line = source[end..].find('\n').map_or(source.len(), |i| end + i);
    if source[line_start..start].trim().is_empty() && source[end..rest_of_line].trim().is_empty() {
        let line_end = (rest_of_line + 1).min(source.len());
        return (line_start as u32, line_end as u32);
    }
    let trailing = source[end..rest_of_line].len() - source[end..rest_of_line].trim_start().len();
    (start as u32, (end + trailing) as u32)
}
