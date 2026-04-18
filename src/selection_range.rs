use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{Position, Range, SelectionRange};

use crate::ast::{ParsedDoc, offset_to_position};

/// Build a selection-range chain for each cursor position.
/// Levels go from innermost to outermost via `parent` links.
pub fn selection_ranges(
    source: &str,
    doc: &ParsedDoc,
    positions: &[Position],
) -> Vec<SelectionRange> {
    let line_starts = doc.line_starts();
    let fr = file_range(source);
    positions
        .iter()
        .map(|pos| build_chain(source, line_starts, &doc.program().stmts, *pos, fr))
        .collect()
}

/// The entire file as a single range.
fn file_range(source: &str) -> Range {
    let lines: Vec<&str> = source.lines().collect();
    let last_line = lines.len().saturating_sub(1) as u32;
    // Use the actual UTF-16 length of the last line rather than u32::MAX.
    // u32::MAX is not LSP-spec-compliant; stricter clients may reject it.
    let last_char = lines
        .last()
        .map(|l| l.chars().map(|c| c.len_utf16() as u32).sum::<u32>())
        .unwrap_or(0);
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            line: last_line,
            character: last_char,
        },
    }
}

/// Build the innermost-to-outermost chain for a cursor position.
fn build_chain(
    source: &str,
    line_starts: &[u32],
    stmts: &[Stmt<'_, '_>],
    pos: Position,
    fr: Range,
) -> SelectionRange {
    let mut ranges: Vec<Range> = Vec::new();
    collect_ranges_stmts(source, line_starts, stmts, pos, &mut ranges);

    // Sort from smallest span to largest (innermost first)
    ranges.sort_by_key(|r| {
        let lines = r.end.line.saturating_sub(r.start.line);
        let chars = if r.start.line == r.end.line {
            r.end.character.saturating_sub(r.start.character)
        } else {
            u32::MAX
        };
        (lines, chars)
    });

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

fn span_range(source: &str, line_starts: &[u32], start: u32, end: u32) -> Range {
    Range {
        start: offset_to_position(source, line_starts, start),
        end: offset_to_position(source, line_starts, end),
    }
}

fn collect_ranges_stmts(
    source: &str,
    line_starts: &[u32],
    stmts: &[Stmt<'_, '_>],
    pos: Position,
    out: &mut Vec<Range>,
) {
    for stmt in stmts {
        collect_ranges_stmt(source, line_starts, stmt, pos, out);
    }
}

fn collect_ranges_stmt(
    source: &str,
    line_starts: &[u32],
    stmt: &Stmt<'_, '_>,
    pos: Position,
    out: &mut Vec<Range>,
) {
    let range = span_range(source, line_starts, stmt.span.start, stmt.span.end);
    match &stmt.kind {
        StmtKind::Function(f) => {
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            collect_ranges_stmts(source, line_starts, &f.body, pos, out);
        }
        StmtKind::Class(c) => {
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            for member in c.members.iter() {
                let m_range = span_range(source, line_starts, member.span.start, member.span.end);
                if !contains(m_range, pos) {
                    continue;
                }
                out.push(m_range);
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    collect_ranges_stmts(source, line_starts, body, pos, out);
                }
            }
        }
        StmtKind::Interface(i) => {
            if contains(range, pos) {
                out.push(range);
                for member in i.members.iter() {
                    let m_range =
                        span_range(source, line_starts, member.span.start, member.span.end);
                    if contains(m_range, pos) {
                        out.push(m_range);
                    }
                }
            }
        }
        StmtKind::Trait(t) => {
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            for member in t.members.iter() {
                let m_range = span_range(source, line_starts, member.span.start, member.span.end);
                if !contains(m_range, pos) {
                    continue;
                }
                out.push(m_range);
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    collect_ranges_stmts(source, line_starts, body, pos, out);
                }
            }
        }
        StmtKind::Enum(e) => {
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            for member in e.members.iter() {
                let m_range = span_range(source, line_starts, member.span.start, member.span.end);
                if !contains(m_range, pos) {
                    continue;
                }
                out.push(m_range);
                if let EnumMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    collect_ranges_stmts(source, line_starts, body, pos, out);
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            if let NamespaceBody::Braced(inner) = &ns.body {
                collect_ranges_stmts(source, line_starts, inner, pos, out);
            }
        }
        StmtKind::If(i) => {
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            collect_ranges_stmt(source, line_starts, i.then_branch, pos, out);
            for ei in i.elseif_branches.iter() {
                collect_ranges_stmt(source, line_starts, &ei.body, pos, out);
            }
            if let Some(e) = &i.else_branch {
                collect_ranges_stmt(source, line_starts, e, pos, out);
            }
        }
        StmtKind::While(w) => {
            if contains(range, pos) {
                out.push(range);
                collect_ranges_stmt(source, line_starts, w.body, pos, out);
            }
        }
        StmtKind::For(f) => {
            if contains(range, pos) {
                out.push(range);
                collect_ranges_stmt(source, line_starts, f.body, pos, out);
            }
        }
        StmtKind::Foreach(f) => {
            if contains(range, pos) {
                out.push(range);
                collect_ranges_stmt(source, line_starts, f.body, pos, out);
            }
        }
        StmtKind::DoWhile(d) => {
            if contains(range, pos) {
                out.push(range);
                collect_ranges_stmt(source, line_starts, d.body, pos, out);
            }
        }
        StmtKind::TryCatch(t) => {
            if !contains(range, pos) {
                return;
            }
            out.push(range);
            collect_ranges_stmts(source, line_starts, &t.body, pos, out);
            for catch in t.catches.iter() {
                collect_ranges_stmts(source, line_starts, &catch.body, pos, out);
            }
            if let Some(finally) = &t.finally {
                collect_ranges_stmts(source, line_starts, finally, pos, out);
            }
        }
        StmtKind::Block(stmts) => {
            if contains(range, pos) {
                out.push(range);
                collect_ranges_stmts(source, line_starts, stmts, pos, out);
            }
        }
        _ => {}
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
        let result = selection_ranges(src, &d, &positions);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn empty_file_returns_file_range() {
        let src = "<?php";
        let d = doc(src);
        let result = selection_ranges(src, &d, &[pos(0, 0)]);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].range.start.line, 0);
    }

    #[test]
    fn cursor_in_function_body_includes_function_range() {
        let src = "<?php\nfunction greet() {\n    echo 'hi';\n}";
        let d = doc(src);
        let result = selection_ranges(src, &d, &[pos(2, 4)]);
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
        let result = selection_ranges(src, &d, &[pos(3, 8)]);
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
        let result = selection_ranges(src, &d, &[pos(1, 0)]);
        assert!(!result.is_empty());
        assert_eq!(result[0].range.start.line, 0);
    }

    #[test]
    fn chain_is_ordered_innermost_to_outermost() {
        let src = "<?php\nclass Foo {\n    public function bar() {\n        echo 1;\n    }\n}";
        let d = doc(src);
        let result = selection_ranges(src, &d, &[pos(3, 8)]);
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
        let result = selection_ranges(src, &d, &[pos(1, 10), pos(2, 10)]);
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
        let result = selection_ranges(src, &d, &[pos(1, 10)]);
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
