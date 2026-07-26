use std::collections::HashMap;
use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{ClassBody, ClassMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView};
use crate::text::word_at_position;

/// Offer "Convert '$var' to instance property" when the cursor is on a local
/// variable inside a non-static method body whose name is not already used by
/// a property or a parameter.
///
/// Produces two edits: inserts `private $prop;` before the first class member
/// (or right inside an empty class body), and replaces every occurrence of
/// `$var` within the method body with `$this->prop`.
pub fn local_to_property_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let cursor = sv.byte_of_position(range.start);

    let var = match word_at_position(source, range.start) {
        Some(w) if w.starts_with('$') && w.len() > 1 && w != "$this" => w,
        _ => return vec![],
    };
    let prop_name = &var[1..];

    let ctx = Ctx {
        var: &var,
        prop_name,
        uri,
    };
    let mut out = Vec::new();
    collect_in_stmts(&doc.program().stmts, source, cursor, sv, &ctx, &mut out);
    out
}

struct Ctx<'a> {
    var: &'a str,
    prop_name: &'a str,
    uri: &'a Url,
}

fn collect_in_stmts<'a>(
    stmts: &[Stmt<'a, 'a>],
    source: &str,
    cursor: u32,
    sv: SourceView<'_>,
    ctx: &Ctx<'_>,
    out: &mut Vec<CodeActionOrCommand>,
) -> bool {
    for stmt in stmts {
        if stmt.span.end < cursor || stmt.span.start > cursor {
            continue;
        }
        match &stmt.kind {
            StmtKind::Class(c) => {
                // Skip if the name is already declared as a property.
                let prop_exists = c.body.members.iter().any(
                    |m| matches!(&m.kind, ClassMemberKind::Property(p) if p.name == ctx.prop_name),
                );
                if prop_exists {
                    return true;
                }

                for member in c.body.members.iter() {
                    let ClassMemberKind::Method(m) = &member.kind else {
                        continue;
                    };
                    let Some(body) = m.body else { continue };
                    if body.span.end < cursor || body.span.start > cursor {
                        continue;
                    }
                    // Static methods would need self::$prop, not $this->prop.
                    if m.is_static {
                        return true;
                    }
                    // Don't convert a parameter into a property — it already has a slot.
                    if m.params.iter().any(|p| p.name == ctx.prop_name) {
                        return true;
                    }

                    let body_start = body.span.start as usize;
                    let body_end = body.span.end as usize;
                    let occurrences =
                        collect_var_occurrences(source, ctx.var, body_start, body_end);
                    if occurrences.is_empty() {
                        return true;
                    }

                    // Bail if the variable is referenced inside a nested closure
                    // or arrow function: blindly rewriting `$var` to `$this->prop`
                    // there would corrupt a `use ($var)` capture clause, and a
                    // `static function`/`static fn` closure has no `$this` at all.
                    // Text-scan replacement can't distinguish "capture clause" from
                    // "body reference" from outside the AST, so the whole action
                    // is withheld rather than emitting a plausibly-broken edit.
                    let nested_closure_spans = collect_nested_closure_spans(&body.stmts);
                    let touches_nested_closure = occurrences.iter().any(|(start, _)| {
                        nested_closure_spans
                            .iter()
                            .any(|(cs, ce)| (*start as u32) >= *cs && (*start as u32) < *ce)
                    });
                    if touches_nested_closure {
                        return true;
                    }

                    if let Some(action) = build_action(source, sv, ctx, &c.body, occurrences) {
                        out.push(action);
                    }
                    return true;
                }
                return true;
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && collect_in_stmts(&inner.stmts, source, cursor, sv, ctx, out)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Byte spans of every `Closure`/`ArrowFunction` expression nested anywhere
/// within `stmts`.
fn collect_nested_closure_spans(stmts: &[Stmt<'_, '_>]) -> Vec<(u32, u32)> {
    struct Collector {
        spans: Vec<(u32, u32)>,
    }
    impl<'arena, 'src> Visitor<'arena, 'src> for Collector {
        fn visit_expr(&mut self, expr: &php_ast::Expr<'arena, 'src>) -> ControlFlow<()> {
            if matches!(&expr.kind, ExprKind::Closure(_) | ExprKind::ArrowFunction(_)) {
                self.spans.push((expr.span.start, expr.span.end));
            }
            walk_expr(self, expr)
        }
    }
    let mut collector = Collector { spans: Vec::new() };
    for stmt in stmts {
        let _ = collector.visit_stmt(stmt);
    }
    collector.spans
}

fn collect_var_occurrences(
    source: &str,
    var: &str,
    body_start: usize,
    body_end: usize,
) -> Vec<(usize, usize)> {
    let body_text = &source[body_start..body_end];
    let var_len = var.len();
    let mut results = Vec::new();
    let mut search_from = 0;

    while let Some(pos) = body_text[search_from..].find(var) {
        let abs = search_from + pos;

        // Reject $$var: character immediately before '$' must not be '$'.
        let before_ok = abs == 0
            || body_text
                .as_bytes()
                .get(abs - 1)
                .is_none_or(|&b| b != b'$' && !b.is_ascii_alphanumeric() && b != b'_');

        let after_ok = body_text
            .as_bytes()
            .get(abs + var_len)
            .is_none_or(|&b| !b.is_ascii_alphanumeric() && b != b'_');

        if before_ok && after_ok {
            results.push((body_start + abs, body_start + abs + var_len));
        }
        search_from = abs + 1;
    }
    results
}

fn build_action(
    source: &str,
    sv: SourceView<'_>,
    ctx: &Ctx<'_>,
    class_body: &ClassBody<'_, '_>,
    occurrences: Vec<(usize, usize)>,
) -> Option<CodeActionOrCommand> {
    let Ctx {
        var,
        prop_name,
        uri,
    } = ctx;
    let replacement = format!("$this->{prop_name}");

    let indent = if let Some(first) = class_body.members.first() {
        line_indent(source, first.span.start as usize)
    } else {
        "    ".to_string()
    };

    let (insert_byte, prop_decl_text) = if let Some(first) = class_body.members.first() {
        let line_start = source[..first.span.start as usize]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        (line_start, format!("{indent}private ${prop_name};\n"))
    } else {
        // Empty class body: insert right after '{'.
        let after_brace = class_body.span.start as usize + 1;
        let next_is_newline = source.as_bytes().get(after_brace).copied() == Some(b'\n');
        if next_is_newline {
            (after_brace + 1, format!("{indent}private ${prop_name};\n"))
        } else {
            (after_brace, format!("\n{indent}private ${prop_name};\n"))
        }
    };

    let insert_pos = sv.position_of(insert_byte as u32);
    let mut edits = Vec::new();

    edits.push(TextEdit {
        range: Range {
            start: insert_pos,
            end: insert_pos,
        },
        new_text: prop_decl_text,
    });

    for (start, end) in &occurrences {
        edits.push(TextEdit {
            range: Range {
                start: sv.position_of(*start as u32),
                end: sv.position_of(*end as u32),
            },
            new_text: replacement.clone(),
        });
    }

    // Sort bottom-to-top so earlier offsets are not invalidated by later inserts.
    edits.sort_by(|a, b| {
        b.range
            .start
            .line
            .cmp(&a.range.start.line)
            .then(b.range.start.character.cmp(&a.range.start.character))
    });

    let mut changes = HashMap::new();
    changes.insert((*uri).clone(), edits);

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Convert '{var}' to instance property"),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

fn line_indent(source: &str, pos: usize) -> String {
    let line_start = source[..pos].rfind('\n').map_or(0, |i| i + 1);
    source[line_start..pos]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect()
}
