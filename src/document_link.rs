/// Document links: clickable paths in require/include expressions and @link/@see docblock tags.
use php_ast::{ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{DocumentLink, Position, Range, Url};

use crate::ast::{ParsedDoc, offset_to_position};

pub fn document_links(uri: &Url, doc: &ParsedDoc, source: &str) -> Vec<DocumentLink> {
    let mut links = Vec::new();
    collect_in_stmts(&doc.program().stmts, source, uri, &mut links);
    collect_docblock_links(source, &mut links);
    links
}

fn collect_in_stmts(stmts: &[Stmt<'_, '_>], source: &str, uri: &Url, out: &mut Vec<DocumentLink>) {
    for stmt in stmts {
        collect_in_stmt(stmt, source, uri, out);
    }
}

fn collect_in_stmt(stmt: &Stmt<'_, '_>, source: &str, uri: &Url, out: &mut Vec<DocumentLink>) {
    match &stmt.kind {
        StmtKind::Expression(e) => collect_in_expr(e, source, uri, out),
        StmtKind::Return(r) => {
            if let Some(v) = r {
                collect_in_expr(v, source, uri, out);
            }
        }
        StmtKind::Echo(exprs) => {
            for expr in exprs.iter() {
                collect_in_expr(expr, source, uri, out);
            }
        }
        StmtKind::Function(f) => collect_in_stmts(&f.body, source, uri, out),
        StmtKind::Class(c) => {
            use php_ast::ClassMemberKind;
            for member in c.members.iter() {
                if let ClassMemberKind::Method(m) = &member.kind {
                    if let Some(body) = &m.body {
                        collect_in_stmts(body, source, uri, out);
                    }
                }
            }
        }
        StmtKind::Namespace(ns) => {
            if let NamespaceBody::Braced(inner) = &ns.body {
                collect_in_stmts(inner, source, uri, out);
            }
        }
        _ => {}
    }
}

fn collect_in_expr(
    expr: &php_ast::Expr<'_, '_>,
    source: &str,
    uri: &Url,
    out: &mut Vec<DocumentLink>,
) {
    if let ExprKind::Include(_, path_expr) = &expr.kind {
        if let Some(link) = link_from_path_expr(path_expr, source, uri) {
            out.push(link);
        }
    }
}

fn link_from_path_expr(
    path_expr: &php_ast::Expr<'_, '_>,
    source: &str,
    uri: &Url,
) -> Option<DocumentLink> {
    let ExprKind::String(s) = &path_expr.kind else {
        return None;
    };
    let raw = s.as_ref();
    if raw.is_empty() {
        return None;
    }
    // span.start points to the opening quote; content starts one byte after
    let quote_offset = path_expr.span.start;
    let content_offset = quote_offset + 1;
    let start = offset_to_position(source, content_offset);
    let end = Position {
        line: start.line,
        character: start.character + raw.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
    };
    let range = Range { start, end };

    let target = if std::path::Path::new(raw).is_absolute() {
        Url::from_file_path(raw).ok()
    } else {
        let base = uri.to_file_path().ok()?;
        let dir = base.parent()?;
        Url::from_file_path(
            dir.join(raw)
                .canonicalize()
                .unwrap_or_else(|_| dir.join(raw)),
        )
        .ok()
    };

    Some(DocumentLink {
        range,
        target,
        tooltip: None,
        data: None,
    })
}

/// Scan source text for `@link` and `@see` tags with HTTP(S) URLs in docblock/line comments.
fn collect_docblock_links(source: &str, out: &mut Vec<DocumentLink>) {
    for (line_idx, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with('*') && !trimmed.starts_with("/**") && !trimmed.starts_with("//") {
            continue;
        }
        for tag in &["@link ", "@see "] {
            if let Some(tag_start) = trimmed.find(tag) {
                let after = trimmed[tag_start + tag.len()..].trim_start();
                if !after.starts_with("http://") && !after.starts_with("https://") {
                    continue;
                }
                let url_str = after.split_whitespace().next().unwrap_or("");
                if url_str.is_empty() {
                    continue;
                }
                if let Ok(target) = Url::parse(url_str) {
                    if let Some(col) = line.find(url_str) {
                        let start = Position {
                            line: line_idx as u32,
                            character: col as u32,
                        };
                        let end = Position {
                            line: line_idx as u32,
                            character: (col + url_str.len()) as u32,
                        };
                        out.push(DocumentLink {
                            range: Range { start, end },
                            target: Some(target),
                            tooltip: None,
                            data: None,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    fn dummy_uri() -> Url {
        Url::parse("file:///project/src/Foo.php").unwrap()
    }

    #[test]
    fn docblock_at_link_produces_link() {
        let src = "<?php\n/** @link https://php.net/array_map */\nfunction foo() {}";
        let d = doc(src);
        let links = document_links(&dummy_uri(), &d, src);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target.as_ref().unwrap().as_str(),
            "https://php.net/array_map"
        );
    }

    #[test]
    fn docblock_see_produces_link() {
        let src = "<?php\n/**\n * @see https://example.com/docs\n */\nfunction bar() {}";
        let d = doc(src);
        let links = document_links(&dummy_uri(), &d, src);
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].target.as_ref().unwrap().as_str(),
            "https://example.com/docs"
        );
    }

    #[test]
    fn non_http_see_is_ignored() {
        let src = "<?php\n/** @see SomeClass::method */\nfunction baz() {}";
        let d = doc(src);
        let links = document_links(&dummy_uri(), &d, src);
        assert!(links.is_empty());
    }
}
