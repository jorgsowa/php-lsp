/// Code action: "Extract variable" — wraps the selected expression in a `$extracted` variable.
use std::collections::HashMap;
use std::ops::ControlFlow;

use php_ast::{
    Expr, Span,
    visitor::{Visitor, walk_expr},
};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::ParsedDoc;
use crate::text::selected_text_range;

/// Offers to extract the selection into `$extracted` only when it is a real `Expr` node in `doc`'s AST — a bare class/function name or a whole class body isn't one, and wrapping it in a variable would produce invalid PHP.
pub fn extract_variable_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    if range.start == range.end {
        return vec![];
    }
    let selected = selected_text_range(source, range);
    if selected.is_empty() || selected.trim().is_empty() {
        return vec![];
    }
    let trimmed = selected.trim();
    if trimmed.starts_with('$')
        && trimmed
            .chars()
            .skip(1)
            .all(|c| c.is_alphanumeric() || c == '_')
    {
        return vec![];
    }

    let sv = doc.view();
    let leading_ws = (selected.len() - selected.trim_start().len()) as u32;
    let trailing_ws = (selected.len() - selected.trim_end().len()) as u32;
    let target = Span::new(
        sv.byte_of_position(range.start) + leading_ws,
        sv.byte_of_position(range.end) - trailing_ws,
    );
    if smallest_expr_span_containing(doc, target).is_none() {
        return vec![];
    }

    let indent = line_indent(source, range.start.line);

    let insert_pos = Position {
        line: range.start.line,
        character: 0,
    };
    let insert_text = format!("{indent}$extracted = {trimmed};\n");

    let replace_text = "$extracted".to_string();

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![
            TextEdit {
                range: Range {
                    start: insert_pos,
                    end: insert_pos,
                },
                new_text: insert_text,
            },
            TextEdit {
                range,
                new_text: replace_text,
            },
        ],
    );

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: "Extract variable".to_string(),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

fn line_indent(source: &str, line: u32) -> String {
    source
        .lines()
        .nth(line as usize)
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_default()
}

/// Span of the smallest `Expr` node in `doc` that contains (or equals) `target`, or `None` if none does.
fn smallest_expr_span_containing(doc: &ParsedDoc, target: Span) -> Option<Span> {
    struct Finder {
        target: Span,
        best: Option<Span>,
    }

    impl<'arena, 'src> Visitor<'arena, 'src> for Finder {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            let span = expr.span;
            if span.start <= self.target.start
                && self.target.end <= span.end
                && self.best.is_none_or(|best| span.len() < best.len())
            {
                self.best = Some(span);
            }
            walk_expr(self, expr)
        }
    }

    let mut finder = Finder { target, best: None };
    for stmt in doc.program().stmts.iter() {
        let _ = finder.visit_stmt(stmt);
    }
    finder.best
}
