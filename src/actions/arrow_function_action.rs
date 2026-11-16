use php_ast::{ArrowFunctionExpr, ClosureExpr, Expr, ExprKind, Span, StmtKind};
use tower_lsp_server::ls_types::{CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri};

use super::cursor_walk::{CursorVisitor, walk_expr_at, walk_stmts_at};
use crate::document::ast::{ParsedDoc, SourceView};

/// Offer "Convert to arrow function" when the cursor is inside a closure whose
/// body is a single `return` statement and no use-by-ref captures are present.
/// The innermost convertible closure at the cursor position wins.
pub fn closure_to_arrow_function_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let cursor = sv.byte_of_position(range.start);
    let mut finder = ClosureFinder {
        source,
        uri,
        sv,
        out: Vec::new(),
    };
    walk_stmts_at(&doc.program().stmts, cursor, &mut finder);
    finder.out
}

struct ClosureFinder<'s> {
    source: &'s str,
    uri: &'s Uri,
    sv: SourceView<'s>,
    out: Vec<CodeActionOrCommand>,
}

impl CursorVisitor for ClosureFinder<'_> {
    fn visit_expr(&mut self, expr: &Expr<'_, '_>, cursor: u32) -> Option<bool> {
        let ExprKind::Closure(c) = &expr.kind else {
            return None;
        };
        // Prefer the innermost convertible closure: recurse into body first.
        if walk_stmts_at(&c.body.stmts, cursor, self) {
            return Some(true);
        }
        let Some(action) = build_action(c, expr.span, self.source, self.uri, self.sv) else {
            return Some(false);
        };
        self.out.push(action);
        Some(true)
    }
}

fn build_action(
    closure: &ClosureExpr<'_, '_>,
    span: Span,
    source: &str,
    uri: &Uri,
    sv: SourceView<'_>,
) -> Option<CodeActionOrCommand> {
    // Reject closures with by-ref `use` captures: arrow functions auto-capture
    // by value and have no equivalent for `use (&$x)`.
    if closure.use_vars.iter().any(|v| v.by_ref) {
        return None;
    }

    // Body must be exactly one `return` statement with an expression.
    if closure.body.stmts.len() != 1 {
        return None;
    }
    let StmtKind::Return(Some(return_expr)) = &closure.body.stmts[0].kind else {
        return None;
    };

    // Generator closures (`yield`) cannot become arrow functions.
    if is_yield(return_expr) {
        return None;
    }

    let new_text = build_arrow_text(closure, span, return_expr, source)?;

    let edit_range = sv.range_of(span);

    let edits = vec![TextEdit {
        range: edit_range,
        new_text,
    }];

    Some(crate::actions::single_file_action(
        "Convert to arrow function",
        CodeActionKind::REFACTOR,
        uri,
        edits,
    ))
}

/// Build the arrow-function source text for the given closure.
///
/// The params are extracted verbatim from source (preserving original
/// formatting); the return type and body expression are sliced the same way.
fn build_arrow_text(
    closure: &ClosureExpr<'_, '_>,
    span: Span,
    return_expr: &Expr<'_, '_>,
    source: &str,
) -> Option<String> {
    let closure_start = span.start as usize;

    // Find the opening `(` of the parameter list by scanning from the closure start.
    let open_paren = source[closure_start..].find('(')? + closure_start;
    let close_paren = find_close_paren(source.as_bytes(), open_paren)?;

    let params_text = &source[open_paren..=close_paren];

    let ret_text = if let Some(rt) = &closure.return_type {
        format!(
            ": {}",
            &source[rt.span.start as usize..rt.span.end as usize]
        )
    } else {
        String::new()
    };

    let expr_text = &source[return_expr.span.start as usize..return_expr.span.end as usize];

    let static_prefix = if closure.is_static { "static " } else { "" };
    let by_ref_marker = if closure.by_ref { "&" } else { "" };

    Some(format!(
        "{static_prefix}fn{by_ref_marker}{params_text}{ret_text} => {expr_text}"
    ))
}

/// Find the index of the `)` that closes the `(` at `open`.
/// Tracks paren depth to handle nested calls in default values.
fn find_close_paren(source: &[u8], open: usize) -> Option<usize> {
    debug_assert_eq!(source[open], b'(');
    let mut depth = 0i32;
    for (i, &b) in source[open..].iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Return `true` if the expression is a `yield` — generator closures cannot
/// be converted to arrow functions.
fn is_yield(expr: &Expr<'_, '_>) -> bool {
    matches!(expr.kind, ExprKind::Yield(_))
}

// ── Arrow function → closure ──────────────────────────────────────────────────

/// Offer "Convert to closure" when the cursor is inside an arrow function.
/// The innermost arrow function at the cursor position wins.
pub fn arrow_function_to_closure_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let cursor = sv.byte_of_position(range.start);
    let mut finder = ArrowFinder {
        source,
        uri,
        sv,
        out: Vec::new(),
    };
    walk_stmts_at(&doc.program().stmts, cursor, &mut finder);
    finder.out
}

struct ArrowFinder<'s> {
    source: &'s str,
    uri: &'s Uri,
    sv: SourceView<'s>,
    out: Vec<CodeActionOrCommand>,
}

impl CursorVisitor for ArrowFinder<'_> {
    fn visit_expr(&mut self, expr: &Expr<'_, '_>, cursor: u32) -> Option<bool> {
        let ExprKind::ArrowFunction(af) = &expr.kind else {
            return None;
        };
        // Prefer the innermost arrow function: recurse into body first.
        if walk_expr_at(af.body, cursor, self) {
            return Some(true);
        }
        let Some(action) =
            build_arrow_to_closure_action(af, expr.span, self.source, self.uri, self.sv)
        else {
            return Some(false);
        };
        self.out.push(action);
        Some(true)
    }
}

fn build_arrow_to_closure_action(
    af: &ArrowFunctionExpr<'_, '_>,
    span: Span,
    source: &str,
    uri: &Uri,
    sv: SourceView<'_>,
) -> Option<CodeActionOrCommand> {
    let new_text = build_closure_text(af, span, source)?;

    let edit_range = sv.range_of(span);

    let edits = vec![TextEdit {
        range: edit_range,
        new_text,
    }];

    Some(crate::actions::single_file_action(
        "Convert to closure",
        CodeActionKind::REFACTOR,
        uri,
        edits,
    ))
}

/// Build the closure source text for the given arrow function.
///
/// Params and return type are extracted verbatim from source; the body
/// expression is wrapped in `{ return <expr>; }`. Arrow functions
/// auto-capture every outer-scope variable they read (by value — they can
/// never write back into it), but a plain closure only sees what it
/// declares in `use (...)`, so any such variable must be captured
/// explicitly or the generated closure fails at runtime with an undefined
/// variable.
fn build_closure_text(af: &ArrowFunctionExpr<'_, '_>, span: Span, source: &str) -> Option<String> {
    let af_start = span.start as usize;

    // Find the opening `(` by scanning past the `fn` keyword (and optional `static `/`&`).
    let open_paren = source[af_start..].find('(')? + af_start;
    let close_paren = find_close_paren(source.as_bytes(), open_paren)?;

    let params_text = &source[open_paren..=close_paren];

    let ret_text = if let Some(rt) = &af.return_type {
        format!(
            ": {}",
            &source[rt.span.start as usize..rt.span.end as usize]
        )
    } else {
        String::new()
    };

    let body_text = &source[af.body.span.start as usize..af.body.span.end as usize];

    let static_prefix = if af.is_static { "static " } else { "" };
    let by_ref_marker = if af.by_ref { "&" } else { "" };

    let param_names = collect_vars(params_text);
    let captured = collect_vars(body_text)
        .into_iter()
        .filter(|v| !param_names.contains(v))
        .collect::<Vec<_>>();
    let use_clause = if captured.is_empty() {
        String::new()
    } else {
        format!(" use ({})", captured.join(", "))
    };

    Some(format!(
        "{static_prefix}function{by_ref_marker}{params_text}{use_clause}{ret_text} {{ return {body_text}; }}"
    ))
}

/// Heuristic text scan for `$var` references, deduped in first-appearance
/// order and excluding `$this` (always implicitly bound in a non-static
/// closure, never valid in a `use` clause). Doesn't distinguish reads from
/// writes or account for nested-closure scoping — matches the level of
/// rigor `collect_vars_in_text` uses elsewhere in the action layer.
fn collect_vars(text: &str) -> Vec<String> {
    let mut vars: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > start {
                let name = &text[start..end];
                let full = format!("${name}");
                // `$this` is always implicitly bound; superglobals
                // (`$_GET`, `$_SERVER`, `$GLOBALS`, ...) are part of the PHP
                // runtime and are a compile-time error in a `use (...)`
                // clause ("Cannot use $_GET as lexical variable as it is a
                // superglobal") — neither belongs in the generated closure's
                // capture list.
                if name != "this"
                    && !crate::editing::rename::is_superglobal(&full)
                    && !vars.contains(&full)
                {
                    vars.push(full);
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    vars
}
