use php_ast::{Expr, ExprKind, Span, Stmt, StmtKind, SwitchStmt};
use tower_lsp_server::ls_types::{CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri};

use super::cursor_walk::{CursorVisitor, walk_stmts_at};
use super::indent_before;
use crate::document::ast::{ParsedDoc, SourceView};

/// Offer "Convert switch to match" when the cursor is inside a `switch` statement
/// where every non-empty case body is a single `return <expr>;`. Falls through
/// (consecutive empty cases) are grouped into comma-separated match arms. Requires
/// a `default` case so that unmatched values throw `UnhandledMatchError` instead
/// of silently falling through — preserving the semantic contract of the original.
pub fn switch_to_match_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let cursor = sv.byte_of_position(range.start);
    let mut finder = SwitchFinder {
        source,
        uri,
        sv,
        out: Vec::new(),
    };
    walk_stmts_at(&doc.program().stmts, cursor, &mut finder);
    finder.out
}

struct SwitchFinder<'s> {
    source: &'s str,
    uri: &'s Uri,
    sv: SourceView<'s>,
    out: Vec<CodeActionOrCommand>,
}

impl CursorVisitor for SwitchFinder<'_> {
    fn visit_stmt(&mut self, stmt: &Stmt<'_, '_>, cursor: u32) -> Option<bool> {
        let StmtKind::Switch(sw) = &stmt.kind else {
            return None;
        };
        // Prefer innermost switch: recurse into case bodies first.
        if sw
            .body
            .cases
            .iter()
            .any(|case| walk_stmts_at(&case.body, cursor, self))
        {
            return Some(true);
        }
        if let Some(action) = build_action(sw, stmt.span, self.source, self.uri, self.sv) {
            self.out.push(action);
        }
        Some(true)
    }
}

fn build_action(
    sw: &SwitchStmt<'_, '_>,
    span: Span,
    source: &str,
    uri: &Uri,
    sv: SourceView<'_>,
) -> Option<CodeActionOrCommand> {
    let new_text = build_match_text(sw, span, source)?;

    let edit_range = sv.range_of(span);

    let edits = vec![TextEdit {
        range: edit_range,
        new_text,
    }];

    Some(crate::actions::single_file_action(
        "Convert switch to match",
        CodeActionKind::REFACTOR,
        uri,
        edits,
    ))
}

struct MatchArm {
    keys: Vec<String>,
    value: String,
}

/// Primitive kinds distinguished by PHP's loose (`==`) vs strict (`===`)
/// comparison — `0 == null`, `0 == false`, and `1 == "1"` all hold loosely
/// but not strictly, while two literals of the same kind compare the same
/// way under both operators for the purposes of this conservative guard.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CaseValueKind {
    Int,
    Float,
    String,
    Bool,
    Null,
}

fn case_value_kind(expr: &Expr<'_, '_>) -> Option<CaseValueKind> {
    match &expr.kind {
        ExprKind::Int(_) => Some(CaseValueKind::Int),
        ExprKind::Float(_) => Some(CaseValueKind::Float),
        ExprKind::String(_) => Some(CaseValueKind::String),
        ExprKind::Bool(_) => Some(CaseValueKind::Bool),
        ExprKind::Null => Some(CaseValueKind::Null),
        _ => None,
    }
}

fn build_match_text(sw: &SwitchStmt<'_, '_>, span: Span, source: &str) -> Option<String> {
    // Reject alternative syntax: switch(): ... endswitch;
    if sw.uses_alternative {
        return None;
    }

    // Require a default arm: without it, switch falls through silently but
    // match would throw UnhandledMatchError, changing observable behavior.
    if !sw.body.cases.iter().any(|c| c.value.is_none()) {
        return None;
    }

    // `switch` compares case values to the subject with loose `==`; `match`
    // uses strict `===`. Case literals of different primitive kinds sharing
    // a subject (e.g. `case 0:`, `case null:`, `case false:`) can match under
    // `==` but not `===`, so converting would silently change behavior.
    // Conservative guard: bail if the classifiable literal case values don't
    // all share one kind. Non-literal case values (constants, calls, ...)
    // aren't classified and don't participate in this check.
    let mut case_kind: Option<CaseValueKind> = None;
    for case in sw.body.cases.iter() {
        let Some(val_expr) = &case.value else {
            continue;
        };
        if let Some(kind) = case_value_kind(val_expr) {
            match case_kind {
                None => case_kind = Some(kind),
                Some(prev) if prev != kind => return None,
                _ => {}
            }
        }
    }

    let mut arms: Vec<MatchArm> = Vec::new();
    let mut pending_keys: Vec<String> = Vec::new();

    for case in sw.body.cases.iter() {
        let key_text = match &case.value {
            Some(val_expr) => {
                source[val_expr.span.start as usize..val_expr.span.end as usize].to_string()
            }
            None => "default".to_string(),
        };

        // Reject if any case body uses a leveled break (e.g. `break 2;`) — it
        // may break out of an enclosing loop, which match cannot replicate.
        if case
            .body
            .iter()
            .any(|s| matches!(s.kind, StmtKind::Break(Some(_))))
        {
            return None;
        }

        // Strip simple `break;` — it's dead code when preceded by `return`,
        // and a break-only body is handled below.
        let non_break: Vec<_> = case
            .body
            .iter()
            .filter(|s| !matches!(s.kind, StmtKind::Break(None)))
            .collect();

        if non_break.is_empty() {
            if case.body.is_empty() {
                // Truly empty body: fall-through to the next case.
                pending_keys.push(key_text);
                continue;
            }
            // Body was all `break;` with no return — can't represent in match.
            return None;
        }

        // Non-empty body: must be exactly one `return <expr>;`.
        if non_break.len() != 1 {
            return None;
        }
        let StmtKind::Return(Some(ret_expr)) = &non_break[0].kind else {
            return None;
        };

        let value_text =
            source[ret_expr.span.start as usize..ret_expr.span.end as usize].to_string();

        pending_keys.push(key_text);
        arms.push(MatchArm {
            keys: std::mem::take(&mut pending_keys),
            value: value_text,
        });
    }

    // Leftover pending_keys means a trailing empty/break-only case at the end.
    if !pending_keys.is_empty() {
        return None;
    }

    if arms.is_empty() {
        return None;
    }

    let base_indent = indent_before(source, span.start as usize);
    let arm_indent = if let Some(first_case) = sw.body.cases.first() {
        indent_before(source, first_case.span.start as usize)
    } else {
        format!("{}    ", base_indent)
    };

    let cond_text = &source[sw.expr.span.start as usize..sw.expr.span.end as usize];

    let mut result = format!("return match ({cond_text}) {{\n");
    for arm in &arms {
        let keys_joined = arm.keys.join(", ");
        result.push_str(&format!("{arm_indent}{keys_joined} => {},\n", arm.value));
    }
    result.push_str(&format!("{base_indent}}};"));

    Some(result)
}
