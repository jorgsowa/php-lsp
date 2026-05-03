use php_ast::{
    CallableCreateKind, ClassDecl, ClassMemberKind, EnumMemberKind, Expr, ExprKind, NamespaceBody,
    Param, PropertyHookBody, Stmt, StmtKind, StringPart,
};
use tower_lsp::lsp_types::{Position, Range, SelectionRange};

use crate::ast::{ParsedDoc, SourceView};

/// Build a selection-range chain for each cursor position.
/// Levels go from innermost to outermost via `parent` links.
pub fn selection_ranges(doc: &ParsedDoc, positions: &[Position]) -> Vec<SelectionRange> {
    let sv = doc.view();
    let fr = file_range(sv);
    positions
        .iter()
        .map(|pos| {
            let byte_off = position_to_byte(sv, *pos);
            build_chain(sv, &doc.program().stmts, byte_off, fr)
        })
        .collect()
}

/// The entire file as a single range.
///
/// Uses the precomputed `line_starts` table to jump to the last line rather
/// than doing an O(file_size) `source.lines().collect()`. Only scans the last
/// line's characters to compute the UTF-16 end column.
fn file_range(sv: SourceView<'_>) -> Range {
    let source = sv.source();
    let line_starts = sv.line_starts();
    if source.is_empty() {
        return Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        };
    }
    let last_line_idx = line_starts.len().saturating_sub(1) as u32;
    let last_line_start = *line_starts.last().unwrap_or(&0) as usize;
    let raw = &source[last_line_start..];
    let line = raw.strip_suffix('\n').unwrap_or(raw);
    let line = line.strip_suffix('\r').unwrap_or(line);
    let last_char: u32 = line.chars().map(|c| c.len_utf16() as u32).sum();
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: last_line_idx,
            character: last_char,
        },
    }
}

/// O(log lines) UTF-16 `Position` → byte offset, via the precomputed
/// `line_starts` table. Scans only the characters on the target line.
fn position_to_byte(sv: SourceView<'_>, pos: Position) -> u32 {
    let source = sv.source();
    let line_starts = sv.line_starts();
    let line_idx = pos.line as usize;
    let line_start = line_starts.get(line_idx).copied().unwrap_or(0) as usize;
    let line_end = line_starts
        .get(line_idx + 1)
        .map(|&s| (s as usize).saturating_sub(1))
        .unwrap_or(source.len());
    let raw = &source[line_start..line_end.min(source.len())];
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    let mut col_utf16: u32 = 0;
    let mut byte_in_line: usize = 0;
    for ch in line.chars() {
        if col_utf16 >= pos.character {
            break;
        }
        col_utf16 += ch.len_utf16() as u32;
        byte_in_line += ch.len_utf8();
    }
    (line_start + byte_in_line) as u32
}

/// Build the innermost-to-outermost chain for a cursor position.
fn build_chain(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    byte_off: u32,
    fr: Range,
) -> SelectionRange {
    let mut spans: Vec<(u32, u32)> = Vec::new();
    collect_spans_stmts(stmts, byte_off, &mut spans);
    // Sort by byte width ascending so the innermost (smallest) span is
    // first. Working in byte space — instead of mapping to LSP ranges
    // first — keeps two same-line-span ranges (e.g. a `while` statement
    // and its inner body block, both ending at the same `}`) correctly
    // ordered: parent ranges always end up *outer* than their children
    // even when the column-based key would tie.
    spans.sort_by_key(|&(s, e)| e - s);
    spans.dedup();
    let ranges: Vec<Range> = spans
        .into_iter()
        .map(|(s, e)| span_range(sv, s, e))
        .collect();
    let mut ranges = ranges;
    ranges.dedup();

    // Ensure file-level range is outermost
    if !ranges.contains(&fr) {
        ranges.push(fr);
    }

    // Build linked chain from outermost inward
    let mut chain: Option<SelectionRange> = None;
    for range in ranges.into_iter().rev() {
        chain = Some(SelectionRange {
            range,
            parent: chain.map(Box::new),
        });
    }

    chain.unwrap_or(SelectionRange {
        range: fr,
        parent: None,
    })
}

#[cfg(test)]
fn contains(range: Range, pos: Position) -> bool {
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

fn span_range(sv: SourceView<'_>, start: u32, end: u32) -> Range {
    Range {
        start: sv.position_of(start),
        end: sv.position_of(end),
    }
}

#[inline]
fn span_contains(start: u32, end: u32, off: u32) -> bool {
    off >= start && off < end
}

#[inline]
fn push_if_contains(s: u32, e: u32, off: u32, out: &mut Vec<(u32, u32)>) -> bool {
    if span_contains(s, e, off) {
        out.push((s, e));
        true
    } else {
        false
    }
}

fn collect_spans_stmts(stmts: &[Stmt<'_, '_>], off: u32, out: &mut Vec<(u32, u32)>) {
    for stmt in stmts {
        collect_spans_stmt(stmt, off, out);
    }
}

fn collect_spans_stmt(stmt: &Stmt<'_, '_>, off: u32, out: &mut Vec<(u32, u32)>) {
    let s = stmt.span.start;
    let e = stmt.span.end;
    if !span_contains(s, e, off) {
        return;
    }
    out.push((s, e));
    match &stmt.kind {
        StmtKind::Function(f) => {
            for p in f.params.iter() {
                collect_spans_param(p, off, out);
            }
            collect_spans_stmts(&f.body, off, out);
        }
        StmtKind::Class(c) => collect_class_members(c, off, out),
        StmtKind::Interface(i) => {
            for member in i.members.iter() {
                collect_class_member(member, off, out);
            }
        }
        StmtKind::Trait(t) => {
            for member in t.members.iter() {
                collect_class_member(member, off, out);
            }
        }
        StmtKind::Enum(en) => {
            for member in en.members.iter() {
                if !push_if_contains(member.span.start, member.span.end, off, out) {
                    continue;
                }
                match &member.kind {
                    EnumMemberKind::Method(m) => {
                        for p in m.params.iter() {
                            collect_spans_param(p, off, out);
                        }
                        if let Some(body) = &m.body {
                            collect_spans_stmts(body, off, out);
                        }
                    }
                    EnumMemberKind::Case(c) => {
                        if let Some(v) = &c.value {
                            collect_spans_expr(v, off, out);
                        }
                    }
                    EnumMemberKind::ClassConst(c) => {
                        collect_spans_expr(&c.value, off, out);
                    }
                    EnumMemberKind::TraitUse(_) => {}
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                collect_spans_stmts(inner, off, out);
            }
        }
        StmtKind::If(i) => {
            collect_spans_expr(&i.condition, off, out);
            collect_spans_stmt(i.then_branch, off, out);
            for ei in i.elseif_branches.iter() {
                if !push_if_contains(ei.span.start, ei.span.end, off, out) {
                    continue;
                }
                collect_spans_expr(&ei.condition, off, out);
                collect_spans_stmt(&ei.body, off, out);
            }
            if let Some(el) = &i.else_branch {
                collect_spans_stmt(el, off, out);
            }
        }
        StmtKind::While(w) => {
            collect_spans_expr(&w.condition, off, out);
            collect_spans_stmt(w.body, off, out);
        }
        StmtKind::For(f) => {
            for e in f.init.iter() {
                collect_spans_expr(e, off, out);
            }
            for e in f.condition.iter() {
                collect_spans_expr(e, off, out);
            }
            for e in f.update.iter() {
                collect_spans_expr(e, off, out);
            }
            collect_spans_stmt(f.body, off, out);
        }
        StmtKind::Foreach(f) => {
            collect_spans_expr(&f.expr, off, out);
            if let Some(k) = &f.key {
                collect_spans_expr(k, off, out);
            }
            collect_spans_expr(&f.value, off, out);
            collect_spans_stmt(f.body, off, out);
        }
        StmtKind::DoWhile(d) => {
            collect_spans_stmt(d.body, off, out);
            collect_spans_expr(&d.condition, off, out);
        }
        StmtKind::Switch(sw) => {
            collect_spans_expr(&sw.expr, off, out);
            for case in sw.cases.iter() {
                if !push_if_contains(case.span.start, case.span.end, off, out) {
                    continue;
                }
                if let Some(v) = &case.value {
                    collect_spans_expr(v, off, out);
                }
                collect_spans_stmts(&case.body, off, out);
            }
        }
        StmtKind::TryCatch(t) => {
            collect_spans_stmts(&t.body, off, out);
            for catch in t.catches.iter() {
                if !push_if_contains(catch.span.start, catch.span.end, off, out) {
                    continue;
                }
                collect_spans_stmts(&catch.body, off, out);
            }
            if let Some(finally) = &t.finally {
                collect_spans_stmts(finally, off, out);
            }
        }
        StmtKind::Block(stmts) => collect_spans_stmts(stmts, off, out),
        StmtKind::Expression(e) => collect_spans_expr(e, off, out),
        StmtKind::Echo(args) => {
            for a in args.iter() {
                collect_spans_expr(a, off, out);
            }
        }
        StmtKind::Return(opt) => {
            if let Some(e) = opt {
                collect_spans_expr(e, off, out);
            }
        }
        StmtKind::Break(opt) | StmtKind::Continue(opt) => {
            if let Some(e) = opt {
                collect_spans_expr(e, off, out);
            }
        }
        StmtKind::Throw(e) => collect_spans_expr(e, off, out),
        StmtKind::Unset(args) => {
            for a in args.iter() {
                collect_spans_expr(a, off, out);
            }
        }
        StmtKind::Const(items) => {
            for item in items.iter() {
                collect_spans_expr(&item.value, off, out);
            }
        }
        StmtKind::StaticVar(items) => {
            for item in items.iter() {
                if let Some(d) = &item.default {
                    collect_spans_expr(d, off, out);
                }
            }
        }
        StmtKind::Declare(d) => {
            for (_, e) in d.directives.iter() {
                collect_spans_expr(e, off, out);
            }
            if let Some(body) = &d.body {
                collect_spans_stmt(body, off, out);
            }
        }
        // Variants whose payload is a name list, raw text, or empty: nothing
        // useful to add beyond the statement span we already pushed.
        StmtKind::Use(_)
        | StmtKind::Global(_)
        | StmtKind::Goto(_)
        | StmtKind::Label(_)
        | StmtKind::HaltCompiler(_)
        | StmtKind::Nop
        | StmtKind::InlineHtml(_)
        | StmtKind::Error => {}
    }
}

fn collect_class_members(c: &ClassDecl<'_, '_>, off: u32, out: &mut Vec<(u32, u32)>) {
    for member in c.members.iter() {
        collect_class_member(member, off, out);
    }
}

fn collect_class_member(
    member: &php_ast::ClassMember<'_, '_>,
    off: u32,
    out: &mut Vec<(u32, u32)>,
) {
    if !push_if_contains(member.span.start, member.span.end, off, out) {
        return;
    }
    match &member.kind {
        ClassMemberKind::Method(m) => {
            for p in m.params.iter() {
                collect_spans_param(p, off, out);
            }
            if let Some(body) = &m.body {
                collect_spans_stmts(body, off, out);
            }
        }
        ClassMemberKind::Property(p) => {
            if let Some(d) = &p.default {
                collect_spans_expr(d, off, out);
            }
            for hook in p.hooks.iter() {
                if !push_if_contains(hook.span.start, hook.span.end, off, out) {
                    continue;
                }
                for hp in hook.params.iter() {
                    collect_spans_param(hp, off, out);
                }
                match &hook.body {
                    PropertyHookBody::Block(stmts) => collect_spans_stmts(stmts, off, out),
                    PropertyHookBody::Expression(e) => collect_spans_expr(e, off, out),
                    PropertyHookBody::Abstract => {}
                }
            }
        }
        ClassMemberKind::ClassConst(c) => collect_spans_expr(&c.value, off, out),
        ClassMemberKind::TraitUse(_) => {}
    }
}

fn collect_spans_param(p: &Param<'_, '_>, off: u32, out: &mut Vec<(u32, u32)>) {
    if !push_if_contains(p.span.start, p.span.end, off, out) {
        return;
    }
    if let Some(d) = &p.default {
        collect_spans_expr(d, off, out);
    }
    for hook in p.hooks.iter() {
        if !push_if_contains(hook.span.start, hook.span.end, off, out) {
            continue;
        }
        match &hook.body {
            PropertyHookBody::Block(stmts) => collect_spans_stmts(stmts, off, out),
            PropertyHookBody::Expression(e) => collect_spans_expr(e, off, out),
            PropertyHookBody::Abstract => {}
        }
    }
}

fn collect_spans_expr(expr: &Expr<'_, '_>, off: u32, out: &mut Vec<(u32, u32)>) {
    let s = expr.span.start;
    let e = expr.span.end;
    if !span_contains(s, e, off) {
        return;
    }
    out.push((s, e));
    match &expr.kind {
        // Atoms — no children.
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bool(_)
        | ExprKind::Null
        | ExprKind::Variable(_)
        | ExprKind::Identifier(_)
        | ExprKind::MagicConst(_)
        | ExprKind::Nowdoc { .. }
        | ExprKind::Error => {}

        ExprKind::InterpolatedString(parts) | ExprKind::ShellExec(parts) => {
            for p in parts.iter() {
                if let StringPart::Expr(inner) = p {
                    collect_spans_expr(inner, off, out);
                }
            }
        }
        ExprKind::Heredoc { parts, .. } => {
            for p in parts.iter() {
                if let StringPart::Expr(inner) = p {
                    collect_spans_expr(inner, off, out);
                }
            }
        }

        ExprKind::VariableVariable(inner) => collect_spans_expr(inner, off, out),
        ExprKind::Assign(a) => {
            collect_spans_expr(a.target, off, out);
            collect_spans_expr(a.value, off, out);
        }
        ExprKind::Binary(b) => {
            collect_spans_expr(b.left, off, out);
            collect_spans_expr(b.right, off, out);
        }
        ExprKind::UnaryPrefix(u) => collect_spans_expr(u.operand, off, out),
        ExprKind::UnaryPostfix(u) => collect_spans_expr(u.operand, off, out),
        ExprKind::Ternary(t) => {
            collect_spans_expr(t.condition, off, out);
            if let Some(then_e) = t.then_expr {
                collect_spans_expr(then_e, off, out);
            }
            collect_spans_expr(t.else_expr, off, out);
        }
        ExprKind::NullCoalesce(n) => {
            collect_spans_expr(n.left, off, out);
            collect_spans_expr(n.right, off, out);
        }
        ExprKind::FunctionCall(f) => {
            collect_spans_expr(f.name, off, out);
            for arg in f.args.iter() {
                if !push_if_contains(arg.span.start, arg.span.end, off, out) {
                    continue;
                }
                collect_spans_expr(&arg.value, off, out);
            }
        }
        ExprKind::Array(elems) => {
            for el in elems.iter() {
                if !push_if_contains(el.span.start, el.span.end, off, out) {
                    continue;
                }
                if let Some(k) = &el.key {
                    collect_spans_expr(k, off, out);
                }
                collect_spans_expr(&el.value, off, out);
            }
        }
        ExprKind::ArrayAccess(a) => {
            collect_spans_expr(a.array, off, out);
            if let Some(idx) = a.index {
                collect_spans_expr(idx, off, out);
            }
        }
        ExprKind::Print(e) => collect_spans_expr(e, off, out),
        ExprKind::Parenthesized(e) => collect_spans_expr(e, off, out),
        ExprKind::Cast(_, e) => collect_spans_expr(e, off, out),
        ExprKind::ErrorSuppress(e) => collect_spans_expr(e, off, out),
        ExprKind::Isset(es) => {
            for e in es.iter() {
                collect_spans_expr(e, off, out);
            }
        }
        ExprKind::Empty(e) => collect_spans_expr(e, off, out),
        ExprKind::Include(_, e) => collect_spans_expr(e, off, out),
        ExprKind::Eval(e) => collect_spans_expr(e, off, out),
        ExprKind::Exit(opt) => {
            if let Some(e) = opt {
                collect_spans_expr(e, off, out);
            }
        }
        ExprKind::Clone(e) => collect_spans_expr(e, off, out),
        ExprKind::New(n) => {
            collect_spans_expr(n.class, off, out);
            for arg in n.args.iter() {
                if !push_if_contains(arg.span.start, arg.span.end, off, out) {
                    continue;
                }
                collect_spans_expr(&arg.value, off, out);
            }
        }
        ExprKind::PropertyAccess(p) | ExprKind::NullsafePropertyAccess(p) => {
            collect_spans_expr(p.object, off, out);
            collect_spans_expr(p.property, off, out);
        }
        ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
            collect_spans_expr(m.object, off, out);
            collect_spans_expr(m.method, off, out);
            for arg in m.args.iter() {
                if !push_if_contains(arg.span.start, arg.span.end, off, out) {
                    continue;
                }
                collect_spans_expr(&arg.value, off, out);
            }
        }
        ExprKind::StaticPropertyAccess(s) | ExprKind::ClassConstAccess(s) => {
            collect_spans_expr(s.class, off, out);
        }
        ExprKind::StaticMethodCall(s) => {
            collect_spans_expr(s.class, off, out);
            for arg in s.args.iter() {
                if !push_if_contains(arg.span.start, arg.span.end, off, out) {
                    continue;
                }
                collect_spans_expr(&arg.value, off, out);
            }
        }
        ExprKind::ClassConstAccessDynamic { class, member }
        | ExprKind::StaticPropertyAccessDynamic { class, member } => {
            collect_spans_expr(class, off, out);
            collect_spans_expr(member, off, out);
        }
        ExprKind::Closure(c) => {
            for p in c.params.iter() {
                collect_spans_param(p, off, out);
            }
            collect_spans_stmts(&c.body, off, out);
        }
        ExprKind::ArrowFunction(a) => {
            for p in a.params.iter() {
                collect_spans_param(p, off, out);
            }
            collect_spans_expr(a.body, off, out);
        }
        ExprKind::Match(m) => {
            collect_spans_expr(m.subject, off, out);
            for arm in m.arms.iter() {
                if !push_if_contains(arm.span.start, arm.span.end, off, out) {
                    continue;
                }
                if let Some(conds) = &arm.conditions {
                    for c in conds.iter() {
                        collect_spans_expr(c, off, out);
                    }
                }
                collect_spans_expr(&arm.body, off, out);
            }
        }
        ExprKind::ThrowExpr(e) => collect_spans_expr(e, off, out),
        ExprKind::Yield(y) => {
            if let Some(k) = y.key {
                collect_spans_expr(k, off, out);
            }
            if let Some(v) = y.value {
                collect_spans_expr(v, off, out);
            }
        }
        ExprKind::AnonymousClass(c) => collect_class_members(c, off, out),
        ExprKind::CallableCreate(c) => match &c.kind {
            CallableCreateKind::Function(e) => collect_spans_expr(e, off, out),
            CallableCreateKind::Method { object, .. } => collect_spans_expr(object, off, out),
            CallableCreateKind::NullsafeMethod { object, .. } => {
                collect_spans_expr(object, off, out)
            }
            CallableCreateKind::StaticMethod { class, .. } => collect_spans_expr(class, off, out),
        },
        ExprKind::CloneWith(target, withs) => {
            collect_spans_expr(target, off, out);
            collect_spans_expr(withs, off, out);
        }
        ExprKind::StaticDynMethodCall(s) => {
            collect_spans_expr(s.class, off, out);
            collect_spans_expr(s.method, off, out);
            for arg in s.args.iter() {
                if !push_if_contains(arg.span.start, arg.span.end, off, out) {
                    continue;
                }
                collect_spans_expr(&arg.value, off, out);
            }
        }
        ExprKind::Omit => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    fn chain_ranges(sr: &SelectionRange) -> Vec<Range> {
        let mut ranges = vec![sr.range];
        let mut current = sr.parent.as_deref();
        while let Some(p) = current {
            ranges.push(p.range);
            current = p.parent.as_deref();
        }
        ranges
    }

    #[test]
    fn returns_one_result_per_position() {
        let src = "<?php\nfunction greet() {}";
        let d = doc(src);
        let positions = vec![pos(1, 10), pos(0, 0)];
        let result = selection_ranges(&d, &positions);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn empty_file_returns_file_range() {
        let src = "<?php";
        let d = doc(src);
        let result = selection_ranges(&d, &[pos(0, 0)]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].range.start.line, 0);
    }

    #[test]
    fn cursor_in_function_body_includes_function_range() {
        let src = "<?php\nfunction greet() {\n    echo 'hi';\n}";
        let d = doc(src);
        let result = selection_ranges(&d, &[pos(2, 4)]);
        let ranges = chain_ranges(&result[0]);
        assert!(
            ranges.iter().any(|r| r.start.line == 1),
            "expected a range starting at line 1 (function), got {:?}",
            ranges
        );
    }

    #[test]
    fn cursor_in_method_body_includes_method_and_class_ranges() {
        let src = "<?php\nclass Foo {\n    public function bar() {\n        echo 1;\n    }\n}";
        let d = doc(src);
        let result = selection_ranges(&d, &[pos(3, 8)]);
        let ranges = chain_ranges(&result[0]);
        assert!(
            ranges.iter().any(|r| r.start.line == 1),
            "expected class-level range at line 1, got {:?}",
            ranges
        );
        assert!(
            ranges.iter().any(|r| r.start.line == 2),
            "expected method-level range at line 2, got {:?}",
            ranges
        );
    }

    #[test]
    fn cursor_outside_all_nodes_returns_file_range_only() {
        let src = "<?php\n// comment\n";
        let d = doc(src);
        let result = selection_ranges(&d, &[pos(1, 0)]);
        assert!(!result.is_empty());
        assert_eq!(result[0].range.start.line, 0);
    }

    #[test]
    fn chain_is_ordered_innermost_to_outermost() {
        let src = "<?php\nclass Foo {\n    public function bar() {\n        echo 1;\n    }\n}";
        let d = doc(src);
        let result = selection_ranges(&d, &[pos(3, 8)]);
        let ranges = chain_ranges(&result[0]);
        for window in ranges.windows(2) {
            let inner = &window[0];
            let outer = &window[1];
            let inner_lines = inner.end.line - inner.start.line;
            let outer_lines = outer.end.line - outer.start.line;
            assert!(
                outer_lines >= inner_lines,
                "outer range should be >= inner range: inner={:?}, outer={:?}",
                inner,
                outer
            );
        }
    }

    #[test]
    fn multiple_positions_are_independent() {
        let src = "<?php\nfunction a() {}\nfunction b() {}";
        let d = doc(src);
        let result = selection_ranges(&d, &[pos(1, 10), pos(2, 10)]);
        assert_eq!(result.len(), 2);
        assert_ne!(result[0].range, result[1].range);
    }

    // ── contains() boundary regression tests ─────────────────────────────────

    #[test]
    fn contains_excludes_exact_end_position() {
        // LSP ranges are half-open [start, end).  The old code used `>` instead
        // of `>=` for the end-character check, so a position exactly at
        // range.end was incorrectly treated as inside the range.
        let range = Range {
            start: Position {
                line: 0,
                character: 4,
            },
            end: Position {
                line: 0,
                character: 9,
            },
        };
        assert!(
            !contains(
                range,
                Position {
                    line: 0,
                    character: 9
                }
            ),
            "exact end position must be outside (half-open range)"
        );
        assert!(
            !contains(
                range,
                Position {
                    line: 0,
                    character: 10
                }
            ),
            "position after end must be outside"
        );
        assert!(
            contains(
                range,
                Position {
                    line: 0,
                    character: 8
                }
            ),
            "position just before end must be inside"
        );
        assert!(
            contains(
                range,
                Position {
                    line: 0,
                    character: 4
                }
            ),
            "start position must be inside"
        );
    }

    #[test]
    fn contains_handles_multiline_range_end() {
        let range = Range {
            start: Position {
                line: 1,
                character: 0,
            },
            end: Position {
                line: 3,
                character: 1,
            },
        };
        // On the end line, character == end.character is outside.
        assert!(!contains(
            range,
            Position {
                line: 3,
                character: 1
            }
        ));
        // On the end line, character < end.character is inside.
        assert!(contains(
            range,
            Position {
                line: 3,
                character: 0
            }
        ));
        // Line between start and end — always inside regardless of character.
        assert!(contains(
            range,
            Position {
                line: 2,
                character: 999
            }
        ));
    }

    #[test]
    fn file_range_end_character_is_actual_line_length_not_u32_max() {
        // The outermost range must use the real UTF-16 column length of the last
        // line, not u32::MAX.  u32::MAX is not LSP-spec-compliant and causes
        // issues with stricter clients.
        let src = "<?php\nfunction hello(): void {}";
        //         line 0             line 1 (30 chars)
        let d = doc(src);
        let result = selection_ranges(&d, &[pos(1, 10)]);
        let ranges = chain_ranges(&result[0]);
        let outermost = ranges.last().expect("should have at least one range");
        assert_ne!(
            outermost.end.character,
            u32::MAX,
            "end character must not be u32::MAX — use real line length"
        );
        // "function hello(): void {}" is 25 chars; the file-level range should end there.
        assert_eq!(
            outermost.end.character, 25,
            "file-level end character should be the actual last-line length"
        );
    }
}
