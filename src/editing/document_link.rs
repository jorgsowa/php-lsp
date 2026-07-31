/// Document links: clickable paths in require/include expressions and @link/@see docblock tags.
use std::ops::ControlFlow;

use php_ast::ExprKind;
use php_ast::visitor::{Visitor, walk_expr};
use tower_lsp_server::ls_types::{DocumentLink, Position, Range, Uri};

use crate::document::ast::{ParsedDoc, SourceView};
use crate::text::byte_to_utf16;

/// RFC3986 allows only unreserved chars in a path; matches what
/// `Uri::from_file_path` itself percent-encodes with.
const PATH_ASCII_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// Builds a `file://` URI directly from a path without requiring it to
/// exist on disk or be OS-absolute — see the caller for why that matters.
fn path_to_file_uri(path: &std::path::Path) -> Option<Uri> {
    let s = path.to_str()?.replace('\\', "/");
    let s = s.strip_prefix('/').unwrap_or(&s);
    let encoded = percent_encoding::utf8_percent_encode(s, PATH_ASCII_SET);
    format!("file:///{encoded}").parse().ok()
}

pub fn document_links(uri: &Uri, doc: &ParsedDoc, _source: &str) -> Vec<DocumentLink> {
    let sv = doc.view();
    let mut collector = LinkCollector {
        sv,
        uri,
        out: Vec::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = collector.visit_stmt(stmt);
    }
    let mut links = collector.out;
    collect_docblock_links(sv.source(), &mut links);
    links
}

/// Walks every statement and expression via the generic `Visitor` trait
/// (rather than hand-matching each `StmtKind`) so `require`/`include` is
/// found no matter how deeply it's nested — inside `if`/loop bodies,
/// `try`/`catch`, `match` arms, traits, enums, etc. A prior hand-rolled
/// version only recursed into a handful of statement kinds and silently
/// missed `require` inside any conditional.
struct LinkCollector<'a> {
    sv: SourceView<'a>,
    uri: &'a Uri,
    out: Vec<DocumentLink>,
}

impl<'arena, 'src> Visitor<'arena, 'src> for LinkCollector<'_> {
    fn visit_expr(&mut self, expr: &php_ast::Expr<'arena, 'src>) -> ControlFlow<()> {
        if let ExprKind::Include(_, path_expr) = &expr.kind
            && let Some(link) = link_from_path_expr(path_expr, self.sv, self.uri)
        {
            self.out.push(link);
        }
        walk_expr(self, expr)
    }
}

fn link_from_path_expr(
    path_expr: &php_ast::Expr<'_, '_>,
    sv: SourceView<'_>,
    uri: &Uri,
) -> Option<DocumentLink> {
    let ExprKind::String(s) = &path_expr.kind else {
        return None;
    };
    let raw: &str = s;
    if raw.is_empty() {
        return None;
    }
    // span.start points to the opening quote; content starts one byte after
    let quote_offset = path_expr.span.start;
    let content_offset = quote_offset + 1;
    let start = sv.position_of(content_offset);
    let end = Position {
        line: start.line,
        character: start.character + raw.chars().map(|c| c.len_utf16() as u32).sum::<u32>(),
    };
    let range = Range { start, end };

    let target = if std::path::Path::new(raw).is_absolute() {
        Uri::from_file_path(raw)
    } else {
        // Resolve relative to the document URI's directory. `ls_types::Uri`
        // has no `.join()` (unlike `url::Url`), so go through the file path.
        let joined = uri
            .to_file_path()
            .and_then(|base| base.parent().map(|dir| dir.join(raw)));
        joined.as_deref().and_then(|p| {
            // `Uri::from_file_path` canonicalizes (requires the path to
            // exist) whenever the joined path isn't OS-absolute. On Windows,
            // `to_file_path` strips the leading `/` of a driveless
            // `file:///foo.php` (no real drive letter, as with a rootless
            // workspace) into a *relative* path, so the join above produces
            // a non-existent relative path and canonicalization fails —
            // even though the URI itself was perfectly well-formed. Fall
            // back to building the `file://` URI directly from the joined
            // path in that case.
            Uri::from_file_path(p).or_else(|| path_to_file_uri(p))
        })
    };

    Some(DocumentLink {
        range,
        target,
        tooltip: None,
        data: None,
    })
}

/// Scan sv.source() text for `@link` and `@see` tags with HTTP(S) URLs in docblock/line comments.
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
                if let Ok(target) = (url_str).parse::<Uri>()
                    && let Some(col) = line.find(url_str)
                {
                    let start = Position {
                        line: line_idx as u32,
                        character: byte_to_utf16(line, col),
                    };
                    let end = Position {
                        line: line_idx as u32,
                        character: byte_to_utf16(line, col + url_str.len()),
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
