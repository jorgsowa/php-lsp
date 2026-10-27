//! The file header: the lines before the first declaration or statement, where
//! `<?php`, `declare`, `namespace` and import `use` statements live.

use php_ast::{NamespaceBody, NamespaceDecl, Stmt, StmtKind};
use tower_lsp_server::ls_types::Position;

use crate::document::ast::ParsedDoc;

/// Number of leading lines of `source` that belong to the header. A `use` inside
/// a class body (trait-use) or after any code is never part of it.
pub(crate) fn header_line_count(source: &str) -> usize {
    let mut in_use_stmt = false;
    let mut in_block_comment = false;
    // A file starts as inline HTML; everything before the open tag is skipped.
    let open_tag_line = source.lines().position(|line| line.contains("<?php"));
    for (idx, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if open_tag_line.is_some_and(|open| idx <= open) {
            continue;
        }
        if in_block_comment {
            in_block_comment = !trimmed.contains("*/");
            continue;
        }
        if in_use_stmt {
            in_use_stmt = !trimmed.contains(';');
            continue;
        }
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || (trimmed.starts_with('#') && !trimmed.starts_with("#["))
        {
            continue;
        }
        if trimmed.starts_with("/*") {
            in_block_comment = !trimmed.contains("*/");
            continue;
        }
        if is_keyword_stmt(trimmed, "declare") || is_keyword_stmt(trimmed, "namespace") {
            continue;
        }
        if is_keyword_stmt(trimmed, "use") {
            in_use_stmt = !trimmed.contains(';');
            continue;
        }
        return idx;
    }
    source.lines().count()
}

/// Lines of the header as `(line number, line)` pairs.
pub(crate) fn header_lines(source: &str) -> impl Iterator<Item = (u32, &str)> {
    source
        .lines()
        .take(header_line_count(source))
        .enumerate()
        .map(|(i, l)| (i as u32, l))
}

/// The namespace that code at a given offset belongs to.
pub(crate) struct NamespaceSegment<'a, 'arena, 'src> {
    pub decl: Option<&'a NamespaceDecl<'arena, 'src>>,
    /// The top-level statements under `decl`, or the whole file without one.
    pub stmts: &'a [Stmt<'arena, 'src>],
    /// Where those statements begin: after `namespace Foo;` or its `{`.
    pub body_start: u32,
}

impl NamespaceSegment<'_, '_, '_> {
    pub fn name(&self) -> String {
        self.decl
            .and_then(|ns| ns.name.as_ref())
            .map(|name| name.to_string_repr().to_string())
            .unwrap_or_default()
    }
}

/// The last namespace starting at or before `offset`, else the first one.
pub(crate) fn namespace_at<'a, 'arena, 'src>(
    stmts: &'a [Stmt<'arena, 'src>],
    offset: u32,
) -> NamespaceSegment<'a, 'arena, 'src> {
    let namespaces: Vec<usize> = stmts
        .iter()
        .enumerate()
        .filter(|(_, stmt)| matches!(stmt.kind, StmtKind::Namespace(_)))
        .map(|(i, _)| i)
        .collect();
    let Some(&i) = namespaces
        .iter()
        .rev()
        .find(|&&i| stmts[i].span.start <= offset)
        .or(namespaces.first())
    else {
        return NamespaceSegment {
            decl: None,
            stmts,
            body_start: 0,
        };
    };
    let StmtKind::Namespace(ns) = &stmts[i].kind else {
        unreachable!("filtered to namespaces");
    };
    match &ns.body {
        NamespaceBody::Braced(block) => NamespaceSegment {
            decl: Some(ns),
            stmts: &block.stmts,
            body_start: block.span.start + 1,
        },
        NamespaceBody::Simple => {
            let end = namespaces
                .iter()
                .find(|&&next| next > i)
                .copied()
                .unwrap_or(stmts.len());
            NamespaceSegment {
                decl: Some(ns),
                stmts: &stmts[i + 1..end],
                body_start: stmts[i].span.end,
            }
        }
    }
}

/// The line on which a new `use` goes for code at `at`: after the last `use` of
/// its namespace accepted by `is_anchor_use`, else after the namespace opening,
/// or after `<?php` and `declare` in a file without one.
pub(crate) fn use_insert_line(
    doc: &ParsedDoc,
    at: Position,
    is_anchor_use: impl Fn(&str) -> bool,
) -> u32 {
    let source = doc.source();
    let view = doc.view();
    let line_after = |offset: u32| view.position_of(offset).line + 1;
    let segment = namespace_at(&doc.program().stmts, view.byte_of_position(at));
    let mut line = if segment.decl.is_some() {
        line_after(segment.body_start)
    } else {
        source
            .find("<?php")
            .map_or(0, |open| line_after(open as u32))
    };
    for stmt in segment.stmts {
        let anchors = match stmt.kind {
            StmtKind::Use(_) => {
                is_anchor_use(source[stmt.span.start as usize..stmt.span.end as usize].trim())
            }
            StmtKind::Declare(_) => segment.decl.is_none(),
            _ => false,
        };
        if anchors {
            line = line.max(line_after(stmt.span.end));
        }
    }
    line
}

fn is_keyword_stmt(trimmed: &str, keyword: &str) -> bool {
    trimmed
        .strip_prefix(keyword)
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_whitespace() || c == '(' || c == '{'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_line(src: &str, cursor_line: u32) -> u32 {
        let doc = ParsedDoc::parse(src);
        use_insert_line(&doc, Position::new(cursor_line, 0), |_| true)
    }

    #[test]
    fn insert_line_in_the_namespace_of_the_cursor() {
        let src = "<?php\nnamespace A;\nuse X;\nclass C {}\n\nnamespace B;\n\nclass D {}\n";
        assert_eq!(insert_line(src, 3), 3);
        assert_eq!(insert_line(src, 7), 6);
    }

    #[test]
    fn insert_line_after_brace_on_its_own_line() {
        let src = "<?php\nnamespace A\n{\n    class C {}\n}\n";
        assert_eq!(insert_line(src, 3), 3);
    }

    #[test]
    fn insert_line_in_second_braced_namespace() {
        let src =
            "<?php\nnamespace A {\n    use X;\n}\nnamespace B {\n    use Y;\n    class D {}\n}\n";
        assert_eq!(insert_line(src, 6), 6);
    }

    #[test]
    fn insert_line_after_declare_and_use_without_namespace() {
        let src = "<?php\ndeclare(strict_types=1);\nuse X;\nclass C {}\n";
        assert_eq!(insert_line(src, 3), 3);
        assert_eq!(insert_line("", 0), 0);
    }

    #[test]
    fn header_stops_at_class() {
        let src = "<?php\nnamespace App;\n\nuse Foo;\n\nclass A {\n    use T;\n}\n";
        assert_eq!(header_line_count(src), 5);
    }

    #[test]
    fn header_skips_docblocks_and_multiline_group_use() {
        let src = "<?php\n/**\n * use Nope;\n */\nuse App\\{\n    A,\n    B\n};\nfunction f() {}\n";
        assert_eq!(header_line_count(src), 8);
    }

    #[test]
    fn header_starts_after_leading_html() {
        let src = "<div>\n<?php\nuse Foo;\n\nclass A {}\n";
        assert_eq!(header_line_count(src), 4);
        assert_eq!(insert_line(src, 0), 3);
        assert_eq!(insert_line("<div>\n<?php\nclass A {}\n", 0), 2);
    }

    #[test]
    fn insert_line_after_declare_not_trait_use() {
        let src = "<?php\n\ndeclare(strict_types=1);\n\nclass A {\n    use T;\n}\n";
        assert_eq!(insert_line(src, 0), 3);
    }

    #[test]
    fn insert_line_after_multiline_group_use() {
        let src = "<?php\nuse App\\{\n    A,\n};\n\nclass C {}\n";
        assert_eq!(insert_line(src, 0), 4);
    }

    #[test]
    fn namespace_separator_is_not_a_namespace_statement() {
        assert!(!is_keyword_stmt("namespaced();", "namespace"));
        assert!(!is_keyword_stmt("namespace\\foo();", "namespace"));
    }
}
