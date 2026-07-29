use php_ast::{NamespaceBody, Span, Stmt, StmtKind, UseKind};
use tower_lsp::lsp_types::{Position, Range, TextEdit};

use crate::document::ast::ParsedDoc;

/// Find every `use` item in `stmts` whose FQN (leading `\` ignored) equals
/// `target`, restricted to `use ClassName` statements (`UseKind::Normal`) —
/// `use function`/`use const` items are never touched by a class file-rename.
/// Returns `(enclosing statement span, item name span)` per match.
///
/// Does not expand group-use (`use App\Model\{User};`) — out of scope,
/// matching the text-scan this replaced. A group member's `Name` span
/// incorrectly extends across the `{` delimiter into the prefix (a parser
/// quirk, not a logical part of the name), so a naive text edit over that
/// span would consume the brace and corrupt the statement; `source` is used
/// to detect and skip this case rather than silently mis-editing it.
fn find_use_items(source: &str, stmts: &[Stmt<'_, '_>], target: &str, out: &mut Vec<(Span, Span)>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => {
                for item in u.uses.iter() {
                    if item.name.to_string_repr().trim_start_matches('\\') != target {
                        continue;
                    }
                    let name_span = item.name.span();
                    let name_text = &source[name_span.start as usize..name_span.end as usize];
                    if name_text.contains(['{', '}']) {
                        continue;
                    }
                    out.push((stmt.span, name_span));
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    find_use_items(source, &inner.stmts, target, out);
                }
            }
            _ => {}
        }
    }
}

/// A name span's text always includes a leading `\` for a fully-qualified
/// name (`\Foo\Bar`) — narrow to just the FQN text so an edit doesn't
/// consume or duplicate the backslash.
fn fqn_span_without_backslash(source: &str, span: Span) -> Span {
    if source.as_bytes().get(span.start as usize) == Some(&b'\\') {
        Span {
            start: span.start + 1,
            end: span.end,
        }
    } else {
        span
    }
}

/// Return `TextEdit`s that delete the entire `use FQN;` line from `doc`.
pub fn delete_use_in_source(doc: &ParsedDoc, fqn: &str) -> Vec<TextEdit> {
    let clean = fqn.trim_start_matches('\\');
    let sv = doc.view();
    let mut matches = Vec::new();
    find_use_items(doc.source(), &doc.program().stmts, clean, &mut matches);

    // A `use A, B;` statement matching on both items would otherwise queue
    // the same line twice.
    let mut seen_lines = std::collections::HashSet::new();
    let mut edits = Vec::new();
    for (stmt_span, _) in matches {
        let line = sv.position_of(stmt_span.start).line;
        if !seen_lines.insert(line) {
            continue;
        }
        // Delete the whole line including its newline.
        edits.push(TextEdit {
            range: Range {
                start: Position { line, character: 0 },
                end: Position {
                    line: line + 1,
                    character: 0,
                },
            },
            new_text: String::new(),
        });
    }
    edits
}

/// Find `use` statements in `doc` that reference `old_fqn` and return
/// `TextEdit`s that replace `old_fqn` with `new_fqn` at each match.
///
/// Handles:
/// - `use OldFqn;`
/// - `use \OldFqn;`
/// - `use OldFqn as Alias;`
pub fn use_edits_in_source(doc: &ParsedDoc, old_fqn: &str, new_fqn: &str) -> Vec<TextEdit> {
    let old = old_fqn.trim_start_matches('\\');
    let new_clean = new_fqn.trim_start_matches('\\');
    let source = doc.source();
    let sv = doc.view();
    let mut matches = Vec::new();
    find_use_items(source, &doc.program().stmts, old, &mut matches);

    matches
        .into_iter()
        .map(|(_, name_span)| {
            let span = fqn_span_without_backslash(source, name_span);
            TextEdit {
                range: Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                },
                new_text: new_clean.to_string(),
            }
        })
        .collect()
}
