use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind};

use crate::ast::{ParsedDoc, offset_to_position};

pub fn folding_ranges(source: &str, doc: &ParsedDoc) -> Vec<FoldingRange> {
    let mut ranges = Vec::new();
    fold_stmts(&doc.program().stmts, source, &mut ranges);
    ranges
}

fn fold_stmts(stmts: &[Stmt<'_, '_>], source: &str, out: &mut Vec<FoldingRange>) {
    for stmt in stmts {
        fold_stmt(stmt, source, out);
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
            for member in i.members.iter() {
                if let ClassMemberKind::Method(_m) = &member.kind {
                    let m_start = offset_to_position(source, member.span.start).line;
                    let m_end = offset_to_position(source, member.span.end).line;
                    push(out, m_start, m_end, None);
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
        assert!(
            ranges.iter().any(|r| r.start_line == 1 && r.end_line == 3),
            "expected function fold (1..3), got {:?}",
            lines(&ranges)
        );
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
        assert!(
            ranges.iter().any(|r| r.start_line == 1),
            "expected interface fold, got {:?}",
            lines(&ranges)
        );
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
}
