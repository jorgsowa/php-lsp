/// Code action: "Inline variable" — replaces all usages of a variable with its
/// initializer expression and removes the assignment statement.
///
/// Only acts when:
/// - The cursor/selection is on or inside a variable name (e.g. `$extracted`).
/// - The variable is written exactly once in its function scope, by a plain
///   `$var = <expr>;` statement that precedes every read and sits in the same
///   block as all of them.
/// - No read is in a position an expression can't take (string interpolation,
///   closure `use`, `isset`/`unset`, an assignment target).
/// - No variable the initializer reads is reassigned before the last read.
/// - No read is passed to a by-reference parameter.
use std::collections::HashMap;
use std::ops::ControlFlow;

use php_ast::{
    Arg, AssignOp, Block, ClosureUseVar, Expr, ExprKind, Span, Stmt, StmtKind,
    visitor::{Visitor, walk_block, walk_expr, walk_stmt},
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
    let sv = doc.view();
    let stmts = &doc.program().stmts;

    let mut refs = Vec::new();
    collect_var_refs_in_scope(
        stmts,
        &var_name[1..],
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

    let mut blockers = InlineBlockers { zones: Vec::new() };
    for stmt in stmts.iter() {
        let _ = blockers.visit_stmt(stmt);
    }
    let usable = |read: &Span| {
        read.start >= found.stmt.end
            && contains(region, *read)
            && !blockers.zones.iter().any(|zone| contains(*zone, *read))
    };
    if reads.is_empty() || !reads.iter().all(usable) {
        return vec![];
    }
    let last_read_end = reads
        .iter()
        .map(|read| read.end)
        .max()
        .unwrap_or(found.stmt.end);
    if found.value_vars.iter().any(|name| {
        written_between(
            stmts,
            name,
            found.value.start,
            found.stmt.end,
            last_read_end,
        )
    }) {
        return vec![];
    }

    let mut sites = ReadSites {
        source,
        reads: &reads,
        args: Vec::new(),
        callees: Vec::new(),
    };
    for stmt in stmts.iter() {
        let _ = sites.visit_stmt(stmt);
    }
    let callees = sites.callees;
    if sites.args.into_iter().any(is_byref_param) {
        return vec![];
    }

    let rhs = &source[found.value.start as usize..found.value.end as usize];
    let mut edits: Vec<TextEdit> = reads
        .iter()
        .map(|read| {
            let new_text = if callees.contains(read) {
                if found.value_is_variable {
                    rhs.to_string()
                } else {
                    format!("({rhs})")
                }
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

/// Whether `$name`, as seen from `scope_at`, is written somewhere in `start..end`.
fn written_between(
    stmts: &[Stmt<'_, '_>],
    name: &str,
    scope_at: u32,
    start: u32,
    end: u32,
) -> bool {
    let mut refs = Vec::new();
    collect_var_refs_in_scope(stmts, name, scope_at as usize, &mut refs);
    refs.iter().any(|(span, kind)| {
        *kind == DocumentHighlightKind::WRITE && span.start >= start && span.start < end
    })
}

struct FoundAssignment {
    stmt: Span,
    value: Span,
    value_is_atomic: bool,
    value_is_variable: bool,
    /// Variables the initializer reads, other than `$this`.
    value_vars: Vec<String>,
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
    found: Option<FoundAssignment>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for AssignmentFinder {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        if let StmtKind::Expression(expr) = &stmt.kind
            && let ExprKind::Assign(assign) = &expr.kind
            && assign.target.span == self.target
        {
            if assign.op == AssignOp::Assign && !assign.by_ref {
                self.found = Some(FoundAssignment {
                    stmt: stmt.span,
                    value: assign.value.span,
                    value_is_atomic: is_atomic(assign.value),
                    value_is_variable: matches!(
                        assign.value.kind,
                        ExprKind::Variable(_) | ExprKind::Parenthesized(_)
                    ),
                    value_vars: variable_names(assign.value),
                    container: match self.containers.last() {
                        None => Some(None),
                        Some(Some(block)) => Some(Some(*block)),
                        Some(None) => None,
                    },
                });
            }
            return ControlFlow::Break(());
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

    fn visit_block(&mut self, block: &Block<'arena, 'src>) -> ControlFlow<()> {
        self.containers.push(Some(block.span));
        let flow = walk_block(self, block);
        self.containers.pop();
        flow
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
}

impl ReadSites<'_> {
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
            ExprKind::StaticMethodCall(call) => self.note_args(call.method.span.start, &call.args),
            ExprKind::New(new) => self.note_args(new.class.span.start, &new.args),
            _ => {}
        }
        walk_expr(self, expr)
    }
}

/// Collects spans where a variable read can't be replaced by an expression.
struct InlineBlockers {
    zones: Vec<Span>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for InlineBlockers {
    fn visit_stmt(&mut self, stmt: &Stmt<'arena, 'src>) -> ControlFlow<()> {
        if let StmtKind::Unset(_) = stmt.kind {
            self.zones.push(stmt.span);
        }
        walk_stmt(self, stmt)
    }

    fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            ExprKind::InterpolatedString(_)
            | ExprKind::Heredoc { .. }
            | ExprKind::ShellExec(_)
            | ExprKind::Isset(_) => self.zones.push(expr.span),
            ExprKind::Assign(assign) if !matches!(assign.target.kind, ExprKind::Variable(_)) => {
                self.zones.push(assign.target.span)
            }
            _ => {}
        }
        walk_expr(self, expr)
    }

    fn visit_closure_use_var(&mut self, use_var: &ClosureUseVar<'src>) -> ControlFlow<()> {
        self.zones.push(use_var.span);
        ControlFlow::Continue(())
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
