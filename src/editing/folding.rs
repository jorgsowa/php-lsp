use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind};

use crate::document::ast::{ParsedDoc, SourceView};

pub fn folding_ranges(_source: &str, doc: &ParsedDoc) -> Vec<FoldingRange> {
    let sv = doc.view();
    let mut ranges = Vec::new();
    fold_stmts(&doc.program().stmts, sv, &mut ranges);
    fold_use_groups(&doc.program().stmts, sv, &mut ranges);
    fold_comments(sv, &mut ranges);
    fold_regions(sv.source(), &mut ranges);
    ranges
}

fn fold_stmts(stmts: &[Stmt<'_, '_>], sv: SourceView<'_>, out: &mut Vec<FoldingRange>) {
    for stmt in stmts {
        fold_stmt(stmt, sv, out);
    }
}

/// Fold the contents of a block body without emitting a fold for the block itself.
/// Used for control-flow statements (`if`, `while`, `for`, `foreach`, `do-while`)
/// where the outer statement already covers the same span as the inner `Block`.
fn fold_body(body: &Stmt<'_, '_>, sv: SourceView<'_>, out: &mut Vec<FoldingRange>) {
    if let StmtKind::Block(stmts) = &body.kind {
        fold_stmts(&stmts.stmts, sv, out);
    }
}

fn fold_stmt(stmt: &Stmt<'_, '_>, sv: SourceView<'_>, out: &mut Vec<FoldingRange>) {
    match &stmt.kind {
        StmtKind::Function(f) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            fold_stmts(&f.body.stmts, sv, out);
        }
        StmtKind::Class(c) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            for member in c.body.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    let m_start = sv.line_of(member.span.start);
                    // member.span.end is exclusive and includes the trailing newline;
                    // subtract 1 so the end line is the line containing the closing `}`.
                    let m_end = sv.line_of(member.span.end.saturating_sub(1));
                    push(out, m_start, m_end, None);
                    if let Some(body) = &m.body {
                        fold_stmts(&body.stmts, sv, out);
                    }
                }
            }
        }
        StmtKind::Interface(i) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            // Interface methods are abstract (no body) — nothing to fold per method.
            for member in i.body.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    let m_start = sv.line_of(member.span.start);
                    let m_end = sv.line_of(member.span.end.saturating_sub(1));
                    push(out, m_start, m_end, None);
                    fold_stmts(&body.stmts, sv, out);
                }
            }
        }
        StmtKind::Trait(t) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            for member in t.body.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    let m_start = sv.line_of(member.span.start);
                    let m_end = sv.line_of(member.span.end.saturating_sub(1));
                    push(out, m_start, m_end, None);
                    if let Some(body) = &m.body {
                        fold_stmts(&body.stmts, sv, out);
                    }
                }
            }
        }
        StmtKind::Enum(e) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            for member in e.body.members.iter() {
                if let EnumMemberKind::Method(m) = &member.kind {
                    let m_start = sv.line_of(member.span.start);
                    let m_end = sv.line_of(member.span.end.saturating_sub(1));
                    push(out, m_start, m_end, None);
                    if let Some(body) = &m.body {
                        fold_stmts(&body.stmts, sv, out);
                    }
                }
            }
        }
        StmtKind::If(i) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            fold_body(i.then_branch, sv, out);
            for ei in i.elseif_branches.iter() {
                fold_body(&ei.body, sv, out);
            }
            if let Some(e) = &i.else_branch {
                fold_body(e, sv, out);
            }
        }
        StmtKind::While(w) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            fold_body(w.body, sv, out);
        }
        StmtKind::For(f) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            fold_body(f.body, sv, out);
        }
        StmtKind::Foreach(f) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            fold_body(f.body, sv, out);
        }
        StmtKind::DoWhile(d) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            fold_body(d.body, sv, out);
        }
        StmtKind::TryCatch(t) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            fold_stmts(&t.body.stmts, sv, out);
            for catch in t.catches.iter() {
                fold_stmts(&catch.body.stmts, sv, out);
            }
            if let Some(finally) = &t.finally {
                fold_stmts(&finally.stmts, sv, out);
            }
        }
        StmtKind::Switch(s) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            for case in s.body.cases.iter() {
                fold_stmts(&case.body, sv, out);
            }
        }
        StmtKind::Block(stmts) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            fold_stmts(&stmts.stmts, sv, out);
        }
        StmtKind::Namespace(ns) => {
            let start_line = sv.line_of(stmt.span.start);
            let end_line = sv.line_of(stmt.span.end);
            push(out, start_line, end_line, None);
            if let NamespaceBody::Braced(inner) = &ns.body {
                fold_stmts(&inner.stmts, sv, out);
            }
        }
        _ => {}
    }
}

/// Fold consecutive top-level `use` statements into a single range.
fn fold_use_groups(stmts: &[Stmt<'_, '_>], sv: SourceView<'_>, out: &mut Vec<FoldingRange>) {
    let mut group_start: Option<u32> = None;
    let mut group_end: u32 = 0;
    for stmt in stmts {
        if matches!(stmt.kind, StmtKind::Use(_)) {
            let line = sv.line_of(stmt.span.start);
            if group_start.is_none() {
                group_start = Some(line);
            }
            group_end = sv.line_of(stmt.span.end);
        } else {
            if let Some(start) = group_start.take() {
                push(out, start, group_end, Some(FoldingRangeKind::Imports));
            }
        }
    }
    if let Some(start) = group_start {
        push(out, start, group_end, Some(FoldingRangeKind::Imports));
    }
}

/// Fold `/* ... */` and `/** ... */` multi-line block comments.
fn fold_comments(sv: SourceView<'_>, out: &mut Vec<FoldingRange>) {
    let bytes = sv.source().as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 1 < len {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            let start_line = line_at(sv, i);
            // find closing */
            let mut j = i + 2;
            while j + 1 < len {
                if bytes[j] == b'*' && bytes[j + 1] == b'/' {
                    let end_line = line_at(sv, j + 1);
                    push(out, start_line, end_line, Some(FoldingRangeKind::Comment));
                    i = j + 2;
                    break;
                }
                j += 1;
            }
            if j + 1 >= len {
                break;
            }
        } else {
            i += 1;
        }
    }
}

/// Fold `// #region` … `// #endregion` pairs.
fn fold_regions(source: &str, out: &mut Vec<FoldingRange>) {
    let mut stack: Vec<u32> = Vec::new();
    for (line_no, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("// #region") || trimmed.starts_with("//region") {
            stack.push(line_no as u32);
        } else if (trimmed.starts_with("// #endregion") || trimmed.starts_with("//endregion"))
            && let Some(start) = stack.pop()
        {
            push(out, start, line_no as u32, Some(FoldingRangeKind::Region));
        }
    }
}

fn line_at(sv: SourceView<'_>, byte_offset: usize) -> u32 {
    sv.line_of(byte_offset as u32)
}

fn push(
    out: &mut Vec<FoldingRange>,
    start_line: u32,
    end_line: u32,
    kind: Option<FoldingRangeKind>,
) {
    if end_line > start_line {
        out.push(FoldingRange {
            start_line,
            start_character: None,
            end_line,
            end_character: None,
            kind,
            collapsed_text: None,
        });
    }
}
