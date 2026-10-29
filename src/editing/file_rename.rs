use php_ast::{NamespaceBody, Span, Stmt, StmtKind, UseKind};
use tower_lsp_server::ls_types::{Position, Range, TextEdit};

use crate::document::ast::ParsedDoc;
use crate::lang::file_header::{namespace_at, use_insert_line};

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

fn namespace_of(fqn: &str) -> &str {
    fqn.trim_start_matches('\\')
        .rsplit_once('\\')
        .map_or("", |(ns, _)| ns)
}

/// Return `TextEdit`s that move the namespace declaration of a renamed class
/// file from the namespace of `old_fqn` to that of `new_fqn`.
pub fn namespace_edits_in_source(doc: &ParsedDoc, old_fqn: &str, new_fqn: &str) -> Vec<TextEdit> {
    let (old_ns, new_ns) = (namespace_of(old_fqn), namespace_of(new_fqn));
    if old_ns.eq_ignore_ascii_case(new_ns) {
        return Vec::new();
    }
    let source = doc.source();
    let sv = doc.view();
    let stmts = &doc.program().stmts;
    let decls: Vec<_> = stmts
        .iter()
        .filter_map(|stmt| match &stmt.kind {
            StmtKind::Namespace(ns) => Some((stmt, ns)),
            _ => None,
        })
        .collect();

    if decls.is_empty() {
        // A global-namespace file gains its declaration after `<?php` and `declare`.
        let line = stmts
            .iter()
            .filter(|s| matches!(s.kind, StmtKind::Declare(_)))
            .map(|s| sv.position_of(s.span.end).line + 1)
            .max()
            .or_else(|| {
                source
                    .find("<?php")
                    .map(|o| sv.position_of(o as u32).line + 1)
            })
            .unwrap_or(0);
        let pos = Position { line, character: 0 };
        return vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text: format!("\nnamespace {new_ns};\n"),
        }];
    }

    decls
        .into_iter()
        .filter_map(|(stmt, ns)| {
            let name = ns.name.as_ref()?;
            if !name
                .to_string_repr()
                .trim_start_matches('\\')
                .eq_ignore_ascii_case(old_ns)
            {
                return None;
            }
            let span = name.span();
            if new_ns.is_empty() && matches!(ns.body, NamespaceBody::Simple) {
                let line = sv.position_of(stmt.span.start).line;
                return Some(TextEdit {
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
            Some(TextEdit {
                range: Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                },
                new_text: new_ns.to_string(),
            })
        })
        .collect()
}

/// An edit inserting `use new_fqn;` when the class reference at `at` resolves
/// only because it shares the namespace of `old_fqn`: unqualified, in that
/// namespace, and not imported.
pub fn same_namespace_import_edit(
    doc: &ParsedDoc,
    at: Position,
    old_fqn: &str,
    new_fqn: &str,
) -> Option<TextEdit> {
    let (old_ns, new_ns) = (namespace_of(old_fqn), namespace_of(new_fqn));
    if old_ns.eq_ignore_ascii_case(new_ns) || new_ns.is_empty() {
        return None;
    }
    let source = doc.source();
    let offset = doc.view().byte_of_position(at);
    if offset > 0 && source.as_bytes().get(offset as usize - 1) == Some(&b'\\') {
        return None;
    }
    let segment = namespace_at(&doc.program().stmts, offset);
    if !segment
        .name()
        .trim_start_matches('\\')
        .eq_ignore_ascii_case(old_ns)
    {
        return None;
    }
    let mut imports = Vec::new();
    find_use_items(
        source,
        segment.stmts,
        old_fqn.trim_start_matches('\\'),
        &mut imports,
    );
    if !imports.is_empty() {
        return None;
    }
    let pos = Position {
        line: use_insert_line(doc, at, |_| true),
        character: 0,
    };
    Some(TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text: format!("use {};\n", new_fqn.trim_start_matches('\\')),
    })
}
