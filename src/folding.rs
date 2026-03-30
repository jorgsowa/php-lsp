use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind};

use crate::ast::{ParsedDoc, offset_to_position};

pub fn folding_ranges(source: &str, doc: &ParsedDoc) -> Vec<FoldingRange> {
    let mut ranges = Vec::new();
    fold_stmts(&doc.program().stmts, source, &mut ranges);
    fold_use_groups(&doc.program().stmts, source, &mut ranges);
    fold_comments(source, &mut ranges);
    fold_regions(source, &mut ranges);
    ranges
}

fn fold_stmts(stmts: &[Stmt<'_, '_>], source: &str, out: &mut Vec<FoldingRange>) {
    for stmt in stmts {
        fold_stmt(stmt, source, out);
    }
}

/// Fold the contents of a block body without emitting a fold for the block itself.
/// Used for control-flow statements (`if`, `while`, `for`, `foreach`, `do-while`)
/// where the outer statement already covers the same span as the inner `Block`.
fn fold_body(body: &Stmt<'_, '_>, source: &str, out: &mut Vec<FoldingRange>) {
    if let StmtKind::Block(stmts) = &body.kind {
        fold_stmts(stmts, source, out);
    }
}

fn fold_stmt(stmt: &Stmt<'_, '_>, source: &str, out: &mut Vec<FoldingRange>) {
    match &stmt.kind {
        StmtKind::Function(f) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            fold_stmts(&f.body, source, out);
        }
        StmtKind::Class(c) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    let m_start = offset_to_position(source, member.span.start).line;
                    // member.span.end is exclusive and includes the trailing newline;
                    // subtract 1 so the end line is the line containing the closing `}`.
                    let m_end = offset_to_position(source, member.span.end.saturating_sub(1)).line;
                    push(out, m_start, m_end, None);
                    if let Some(body) = &m.body {
                        fold_stmts(body, source, out);
                    }
                }
            }
        }
        StmtKind::Interface(i) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            // Interface methods are abstract (no body) — nothing to fold per method.
            for member in i.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind
                    && let Some(body) = &m.body
                {
                    let m_start = offset_to_position(source, member.span.start).line;
                    let m_end = offset_to_position(source, member.span.end.saturating_sub(1)).line;
                    push(out, m_start, m_end, None);
                    fold_stmts(body, source, out);
                }
            }
        }
        StmtKind::Trait(t) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            for member in t.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    let m_start = offset_to_position(source, member.span.start).line;
                    let m_end = offset_to_position(source, member.span.end.saturating_sub(1)).line;
                    push(out, m_start, m_end, None);
                    if let Some(body) = &m.body {
                        fold_stmts(body, source, out);
                    }
                }
            }
        }
        StmtKind::Enum(_e) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
        }
        StmtKind::If(i) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            fold_body(i.then_branch, source, out);
            for ei in i.elseif_branches.iter() {
                fold_body(&ei.body, source, out);
            }
            if let Some(e) = &i.else_branch {
                fold_body(e, source, out);
            }
        }
        StmtKind::While(w) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            fold_body(w.body, source, out);
        }
        StmtKind::For(f) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            fold_body(f.body, source, out);
        }
        StmtKind::Foreach(f) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            fold_body(f.body, source, out);
        }
        StmtKind::DoWhile(d) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            fold_body(d.body, source, out);
        }
        StmtKind::TryCatch(t) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            fold_stmts(&t.body, source, out);
            for catch in t.catches.iter() {
                fold_stmts(&catch.body, source, out);
            }
            if let Some(finally) = &t.finally {
                fold_stmts(finally, source, out);
            }
        }
        StmtKind::Block(stmts) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            fold_stmts(stmts, source, out);
        }
        StmtKind::Namespace(ns) => {
            let start_line = offset_to_position(source, stmt.span.start).line;
            let end_line = offset_to_position(source, stmt.span.end).line;
            push(out, start_line, end_line, None);
            if let NamespaceBody::Braced(inner) = &ns.body {
                fold_stmts(inner, source, out);
            }
        }
        _ => {}
    }
}

/// Fold consecutive top-level `use` statements into a single range.
fn fold_use_groups(stmts: &[Stmt<'_, '_>], source: &str, out: &mut Vec<FoldingRange>) {
    let mut group_start: Option<u32> = None;
    let mut group_end: u32 = 0;
    for stmt in stmts {
        if matches!(stmt.kind, StmtKind::Use(_)) {
            let line = offset_to_position(source, stmt.span.start).line;
            if group_start.is_none() {
                group_start = Some(line);
            }
            group_end = offset_to_position(source, stmt.span.end).line;
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
fn fold_comments(source: &str, out: &mut Vec<FoldingRange>) {
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i + 1 < len {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            let start_line = line_at(source, i);
            // find closing */
            let mut j = i + 2;
            while j + 1 < len {
                if bytes[j] == b'*' && bytes[j + 1] == b'/' {
                    let end_line = line_at(source, j + 1);
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

fn line_at(source: &str, byte_offset: usize) -> u32 {
    source[..byte_offset]
        .bytes()
        .filter(|&b| b == b'\n')
        .count() as u32
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    fn lines(ranges: &[FoldingRange]) -> Vec<(u32, u32)> {
        ranges.iter().map(|r| (r.start_line, r.end_line)).collect()
    }

    #[test]
    fn folds_top_level_function() {
        let src = "<?php\nfunction greet(): void {\n    echo 'hi';\n}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert_eq!(
            ranges.len(),
            1,
            "expected exactly 1 fold for top-level function, got {:?}",
            lines(&ranges)
        );
        assert_eq!(ranges[0].start_line, 1);
        assert_eq!(ranges[0].end_line, 3);
    }

    #[test]
    fn folds_class_and_its_method() {
        let src =
            "<?php\nclass Foo {\n    public function bar(): void {\n        echo 1;\n    }\n}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        let ls = lines(&ranges);
        assert!(
            ls.contains(&(1, 5)),
            "expected class fold (1..5), got {:?}",
            ls
        );
        assert!(
            ls.contains(&(2, 4)),
            "expected method fold (2..4), got {:?}",
            ls
        );
    }

    #[test]
    fn folds_interface() {
        let src = "<?php\ninterface Countable {\n    public function count(): int;\n}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert_eq!(
            ranges.len(),
            1,
            "expected exactly 1 fold for interface, got {:?}",
            lines(&ranges)
        );
        assert_eq!(ranges[0].start_line, 1);
        assert_eq!(ranges[0].end_line, 3);
    }

    #[test]
    fn folds_trait_and_its_method() {
        let src = "<?php\ntrait Loggable {\n    public function log(): void {\n        echo 'log';\n    }\n}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        let ls = lines(&ranges);
        assert!(
            ls.contains(&(1, 5)),
            "expected trait fold (1..5), got {:?}",
            ls
        );
        assert!(
            ls.contains(&(2, 4)),
            "expected method fold (2..4), got {:?}",
            ls
        );
    }

    #[test]
    fn folds_braced_namespace() {
        let src = "<?php\nnamespace App {\n    function boot(): void {\n        return;\n    }\n}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        let ls = lines(&ranges);
        assert!(
            ls.contains(&(1, 5)),
            "expected namespace fold (1..5), got {:?}",
            ls
        );
        assert!(
            ls.contains(&(2, 4)),
            "expected function fold (2..4), got {:?}",
            ls
        );
    }

    #[test]
    fn single_line_construct_produces_no_fold() {
        let src = "<?php\nfunction f(): void { echo 1; }";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert!(
            ranges.is_empty(),
            "single-line function should not fold, got {:?}",
            ranges
        );
    }

    #[test]
    fn no_folds_for_empty_file() {
        let src = "<?php";
        let d = doc(src);
        assert!(folding_ranges(src, &d).is_empty());
    }

    #[test]
    fn folds_if_statement() {
        let src = "<?php\nif (true) {\n    echo 1;\n}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert_eq!(
            ranges.len(),
            1,
            "expected exactly 1 fold for if, got {:?}",
            lines(&ranges)
        );
        assert_eq!(ranges[0].start_line, 1);
        assert_eq!(ranges[0].end_line, 3);
    }

    #[test]
    fn folds_foreach_statement() {
        let src = "<?php\nforeach ($arr as $v) {\n    echo $v;\n}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert_eq!(
            ranges.len(),
            1,
            "expected exactly 1 fold for foreach, got {:?}",
            lines(&ranges)
        );
        assert_eq!(ranges[0].start_line, 1);
        assert_eq!(ranges[0].end_line, 3);
    }

    #[test]
    fn folds_try_catch() {
        let src = "<?php\ntry {\n    foo();\n} catch (\\Exception $e) {\n    bar();\n}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert!(
            ranges.iter().any(|r| r.start_line == 1 && r.end_line == 5),
            "expected try-catch fold (1..5), got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn folds_multiline_doc_comment() {
        let src = "<?php\n/**\n * A docblock.\n * @param int $x\n */\nfunction f(int $x): void {}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        let comment_fold = ranges
            .iter()
            .find(|r| r.kind == Some(FoldingRangeKind::Comment));
        assert!(
            comment_fold.is_some(),
            "expected a comment fold, got {:?}",
            lines(&ranges)
        );
        let cf = comment_fold.unwrap();
        assert_eq!(cf.start_line, 1);
        assert_eq!(cf.end_line, 4);
    }

    #[test]
    fn folds_region_endregion() {
        let src = "<?php\n// #region Auth\n$x = 1;\n$y = 2;\n// #endregion";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert!(
            ranges
                .iter()
                .any(|r| r.kind == Some(FoldingRangeKind::Region)
                    && r.start_line == 1
                    && r.end_line == 4),
            "expected region fold (1..4), got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn folds_consecutive_use_statements() {
        let src = "<?php\nuse Foo\\Bar;\nuse Foo\\Baz;\nuse Foo\\Qux;\n\nclass A {}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert!(
            ranges
                .iter()
                .any(|r| r.kind == Some(FoldingRangeKind::Imports) && r.start_line == 1),
            "expected imports fold, got {:?}",
            lines(&ranges)
        );
    }

    #[test]
    fn nested_folds_both_returned() {
        // A class containing a method should produce BOTH a class fold and a method fold.
        let src =
            "<?php\nclass Outer {\n    public function inner(): void {\n        echo 1;\n    }\n}";
        // Line 1 = class, Line 2 = method, Line 4 = }, Line 5 = }
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        let ls = lines(&ranges);
        assert!(
            ls.contains(&(1, 5)),
            "expected class fold (1..5), got {:?}",
            ls
        );
        assert!(
            ls.contains(&(2, 4)),
            "expected method fold (2..4), got {:?}",
            ls
        );
        assert_eq!(
            ranges.len(),
            2,
            "expected exactly 2 fold ranges (class + method), got {:?}",
            ls
        );
    }

    #[test]
    fn single_line_function_not_folded() {
        // `function f() {}` is on a single line — no fold range should be produced.
        let src = "<?php\nfunction f() {}";
        let d = doc(src);
        let ranges = folding_ranges(src, &d);
        assert!(
            ranges.is_empty(),
            "single-line function should produce NO fold range, got {:?}",
            lines(&ranges)
        );
    }
}
