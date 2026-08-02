//! Detection of a string-literal argument to a bare Laravel helper call
//! (`env('KEY')`, `config('a.b')`, `view('a.b')`, `trans('a.b')`,
//! `route('name')`).
//!
//! [`call_string_arg`] and [`find_call_sites`] walk the AST — these calls
//! only ever run on complete, saved code (hover, goto-definition,
//! find-references), so there is no unterminated-literal case to design
//! around. [`call_string_prefix`] stays a text scan: it backs completion,
//! which must work while the string is still being typed and the closing
//! quote may not exist yet — a shape the AST can't represent.

use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{Expr, ExprKind, Span};
use tower_lsp_server::ls_types::{Position, Range};

use crate::document::ast::ParsedDoc;
use crate::text::utf16_offset_to_byte;

/// A bare call's string-literal first argument, found by walking the AST.
struct StringArgCall {
    /// Full token span, quotes included — used to test whether the cursor
    /// sits inside the literal.
    token_span: Span,
    /// Decoded content (quotes excluded, escapes resolved).
    content: String,
    /// Byte span of the raw content, quotes excluded — for building the
    /// returned `Range`.
    content_span: Span,
}

/// Byte span of a string literal's content (quotes excluded), found by
/// locating the matching quote characters within `span` (the full token,
/// quotes included). Search rather than assume the first/last byte so the
/// legacy `b'...'`/`b"..."` byte-string prefix doesn't throw off the count.
pub(crate) fn content_span(source: &str, span: Span) -> Option<Span> {
    let text = source.get(span.start as usize..span.end as usize)?;
    let bytes = text.as_bytes();
    let quote = *bytes.iter().find(|&&b| b == b'\'' || b == b'"')?;
    let start_rel = bytes.iter().position(|&b| b == quote)?;
    let end_rel = bytes.iter().rposition(|&b| b == quote)?;
    if end_rel <= start_rel {
        return None;
    }
    Some(Span::new(
        span.start + start_rel as u32 + 1,
        span.start + end_rel as u32,
    ))
}

/// Every bare call to one of `names` whose first argument is a plain
/// (non-interpolated) string literal.
fn collect_string_arg_calls(doc: &ParsedDoc, names: &[&str]) -> Vec<StringArgCall> {
    struct Collector<'a> {
        source: &'a str,
        names: &'a [&'a str],
        out: Vec<StringArgCall>,
    }

    impl<'arena, 'src> Visitor<'arena, 'src> for Collector<'_> {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if let ExprKind::FunctionCall(f) = &expr.kind
                && let Some(name) = f.name.name_str()
                && self.names.iter().any(|n| n.eq_ignore_ascii_case(name))
                && let Some(arg) = f.args.first()
                && arg.name.is_none()
                && !arg.unpack
                && let Some(arg_value) = &arg.value
                && let ExprKind::String(s) = &arg_value.kind
                && let Some(cspan) = content_span(self.source, arg_value.span)
            {
                self.out.push(StringArgCall {
                    token_span: arg_value.span,
                    content: (*s).to_string(),
                    content_span: cspan,
                });
            }
            walk_expr(self, expr)
        }
    }

    let mut collector = Collector {
        source: doc.source(),
        names,
        out: Vec::new(),
    };
    for stmt in doc.program().stmts.iter() {
        let _ = collector.visit_stmt(stmt);
    }
    collector.out
}

/// Full string-literal content (quotes excluded) and its `Range`, when the
/// cursor sits anywhere inside a *closed* string literal that is the first
/// argument of a bare call to one of `names` — e.g. cursor anywhere inside
/// `'APP_NAME'` in `env('APP_NAME')`.
pub(crate) fn call_string_arg(
    doc: &ParsedDoc,
    position: Position,
    names: &[&str],
) -> Option<(String, Range)> {
    let sv = doc.view();
    let offset = sv.byte_of_position(position);
    collect_string_arg_calls(doc, names)
        .into_iter()
        .find(|c| c.token_span.start <= offset && offset < c.token_span.end)
        .map(|c| (c.content, sv.range_of(c.content_span)))
}

/// Typed prefix (from the opening quote up to the cursor), when the cursor
/// sits inside a string literal — closed or not — immediately following a
/// bare call to one of `names`. Used for completion, where the closing quote
/// may not exist yet.
pub(crate) fn call_string_prefix(
    source: &str,
    position: Position,
    names: &[&str],
) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let line = *lines.get(position.line as usize)?;
    let byte_col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..byte_col];
    let quote_pos = before.rfind(['\'', '"'])?;
    if !preceded_by_call_wrapped(&lines, position.line as usize, &before[..quote_pos], names) {
        return None;
    }
    Some(before[quote_pos + 1..].to_string())
}

/// Every string-literal argument to a bare call to one of `names` anywhere
/// in `doc` whose content equals `target`, with its `Range`. Used to sweep a
/// file for Laravel string-key usages once the key is already known
/// (find-references), as opposed to `call_string_arg`'s single
/// cursor-position lookup.
pub(crate) fn find_call_sites(doc: &ParsedDoc, names: &[&str], target: &str) -> Vec<Range> {
    let sv = doc.view();
    collect_string_arg_calls(doc, names)
        .into_iter()
        .filter(|c| c.content == target)
        .map(|c| sv.range_of(c.content_span))
        .collect()
}

/// Every string-literal argument to a bare call to one of `names` anywhere in
/// `doc`, with its decoded content and `Range` — unlike `find_call_sites`,
/// not filtered to a single already-known target. Used to build document
/// links for a whole file in one AST walk.
pub(crate) fn find_all_calls(doc: &ParsedDoc, names: &[&str]) -> Vec<(String, Range)> {
    let sv = doc.view();
    collect_string_arg_calls(doc, names)
        .into_iter()
        .map(|c| (c.content, sv.range_of(c.content_span)))
        .collect()
}

/// Same as `preceded_by_call`, but also recognizes a wrapped call — one
/// where the opening `name(` sits on an earlier line than the string
/// argument, a common shape after formatter line-wrapping for long
/// route/view/translation names:
/// ```php
/// route(
///     'admin.dashboard'
/// );
/// ```
/// Only kicks in when there is nothing but whitespace before the quote on
/// its own line — otherwise `before_quote` already carries the real answer.
fn preceded_by_call_wrapped(
    lines: &[&str],
    line_idx: usize,
    before_quote: &str,
    names: &[&str],
) -> bool {
    if preceded_by_call(before_quote, names) {
        return true;
    }
    if !before_quote.trim().is_empty() {
        return false;
    }
    let mut i = line_idx;
    while i > 0 {
        i -= 1;
        let prev = lines[i];
        if prev.trim().is_empty() {
            continue;
        }
        return preceded_by_call(prev, names);
    }
    false
}

/// Whether `before_quote` (the line text up to, but excluding, the opening
/// quote) ends with a bare call to one of `names` — `env(`, `config(`, etc.
/// — at a word boundary, so `getenv(` doesn't match the `env` pattern.
fn preceded_by_call(before_quote: &str, names: &[&str]) -> bool {
    let Some(rest) = before_quote.trim_end().strip_suffix('(') else {
        return false;
    };
    let rest = rest.trim_end();
    names.iter().any(|name| {
        rest.len() >= name.len()
            && rest[rest.len() - name.len()..].eq_ignore_ascii_case(name)
            && word_boundary_before(rest, rest.len() - name.len())
    })
}

fn word_boundary_before(s: &str, idx: usize) -> bool {
    idx == 0 || !matches!(s.as_bytes()[idx - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV: &[&str] = &["env"];

    fn parse(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    #[test]
    fn call_string_arg_matches_env_call() {
        let doc = parse("<?php\n$x = env('APP_NAME');\n");
        // Cursor inside "APP_NAME".
        let pos = Position {
            line: 1,
            character: 15,
        };
        let (content, range) = call_string_arg(&doc, pos, ENV).unwrap();
        assert_eq!(content, "APP_NAME");
        assert_eq!(
            range.start,
            Position {
                line: 1,
                character: 10
            }
        );
        assert_eq!(
            range.end,
            Position {
                line: 1,
                character: 18
            }
        );
    }

    #[test]
    fn call_string_arg_rejects_unrelated_call() {
        let doc = parse("<?php\n$x = getenv('APP_NAME');\n");
        let pos = Position {
            line: 1,
            character: 18,
        };
        assert!(call_string_arg(&doc, pos, ENV).is_none());
    }

    #[test]
    fn call_string_arg_rejects_plain_string_containing_pattern_textually() {
        let doc = parse("<?php\n$x = 'env(APP_NAME)';\n");
        let pos = Position {
            line: 1,
            character: 12,
        };
        assert!(call_string_arg(&doc, pos, ENV).is_none());
    }

    #[test]
    fn call_string_arg_allows_whitespace_before_paren_and_quote() {
        let doc = parse("<?php\n$x = env( 'APP_NAME' );\n");
        let pos = Position {
            line: 1,
            character: 16,
        };
        let (content, _) = call_string_arg(&doc, pos, ENV).unwrap();
        assert_eq!(content, "APP_NAME");
    }

    #[test]
    fn call_string_prefix_returns_typed_text() {
        let src = "<?php\nenv('APP_N";
        let pos = Position {
            line: 1,
            character: 11,
        };
        assert_eq!(call_string_prefix(src, pos, ENV).as_deref(), Some("APP_N"));
    }

    #[test]
    fn call_string_prefix_none_for_other_call() {
        let src = "<?php\nconfig('APP_N";
        let pos = Position {
            line: 1,
            character: 14,
        };
        assert!(call_string_prefix(src, pos, ENV).is_none());
    }

    #[test]
    fn call_string_prefix_empty_right_after_quote() {
        let src = "<?php\nenv('";
        let pos = Position {
            line: 1,
            character: 5,
        };
        assert_eq!(call_string_prefix(src, pos, ENV).as_deref(), Some(""));
    }

    #[test]
    fn find_call_sites_collects_every_matching_call_across_lines() {
        let doc =
            parse("<?php\n$a = env('APP_NAME');\n$b = env('APP_NAME');\n$c = env('OTHER');\n");
        let sites = find_call_sites(&doc, ENV, "APP_NAME");
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].start.line, 1);
        assert_eq!(sites[1].start.line, 2);
    }

    #[test]
    fn find_call_sites_ignores_unrelated_calls_and_keys() {
        let doc = parse("<?php\n$a = getenv('APP_NAME');\n$b = env('OTHER');\n");
        assert!(find_call_sites(&doc, ENV, "APP_NAME").is_empty());
    }

    #[test]
    fn find_call_sites_empty_for_no_matches() {
        let doc = parse("<?php\necho 'hello';\n");
        assert!(find_call_sites(&doc, ENV, "APP_NAME").is_empty());
    }

    #[test]
    fn call_string_arg_matches_wrapped_call() {
        let doc = parse("<?php\nenv(\n    'APP_NAME'\n);\n");
        // Cursor inside "APP_NAME" on its own line.
        let pos = Position {
            line: 2,
            character: 8,
        };
        let (content, _) = call_string_arg(&doc, pos, ENV).unwrap();
        assert_eq!(content, "APP_NAME");
    }

    #[test]
    fn call_string_arg_wrapped_call_skips_blank_lines() {
        let doc = parse("<?php\nenv(\n\n    'APP_NAME'\n);\n");
        let pos = Position {
            line: 3,
            character: 8,
        };
        let (content, _) = call_string_arg(&doc, pos, ENV).unwrap();
        assert_eq!(content, "APP_NAME");
    }

    #[test]
    fn call_string_arg_wrapped_call_rejects_unrelated_call() {
        let doc = parse("<?php\ngetenv(\n    'APP_NAME'\n);\n");
        let pos = Position {
            line: 2,
            character: 8,
        };
        assert!(call_string_arg(&doc, pos, ENV).is_none());
    }

    #[test]
    fn find_call_sites_matches_wrapped_call() {
        let doc = parse("<?php\nenv(\n    'APP_NAME'\n);\n");
        let sites = find_call_sites(&doc, ENV, "APP_NAME");
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].start.line, 2);
    }

    #[test]
    fn find_all_calls_returns_every_call_regardless_of_content() {
        let doc =
            parse("<?php\n$a = env('APP_NAME');\n$b = env('DB_HOST');\n$c = getenv('OTHER');\n");
        let calls = find_all_calls(&doc, ENV);
        let contents: Vec<&str> = calls.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(contents, vec!["APP_NAME", "DB_HOST"]);
    }
}
