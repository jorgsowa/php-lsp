/// Code action: "Generate validation rules from $request usages" — offered
/// when the cursor is inside an array literal that's either the sole
/// argument to `$request->validate([...])`/`$this->validate([...])`, or the
/// `return [...]` of a method named `rules()`. Inserts a `'field' =>
/// 'required'` stub for every field name harvested from
/// `->input()`/`->get()`/`->post()`/`->query()` calls elsewhere — the same
/// enclosing method for the `validate()` case, or anywhere else in the same
/// class for `rules()` (a FormRequest's own `rules()` body has no `$request`
/// variable to call these on; a `prepareForValidation()`-style hook using
/// `$this->input(...)` is the realistic source there).
use std::collections::HashMap;
use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{ArrayElement, ClassMemberKind, Expr, ExprKind, NamespaceBody, Span, Stmt, StmtKind};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView};
use crate::laravel::request_fields::{Receiver, harvest_fields};

pub fn generate_validation_rules_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
    is_laravel: bool,
) -> Vec<CodeActionOrCommand> {
    if !is_laravel {
        return Vec::new();
    }
    let sv = doc.view();
    let cursor = sv.byte_of_position(range.start);
    let mut out = Vec::new();
    collect(&doc.program().stmts, source, sv, cursor, uri, &mut out);
    out
}

fn collect<'a>(
    stmts: &[Stmt<'a, 'a>],
    source: &str,
    sv: SourceView<'_>,
    cursor: u32,
    uri: &Uri,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                if !(stmt.span.start <= cursor && cursor <= stmt.span.end) {
                    continue;
                }
                for member in c.body.members.iter() {
                    let ClassMemberKind::Method(m) = &member.kind else {
                        continue;
                    };
                    let Some(body) = &m.body else { continue };
                    if !(body.span.start <= cursor && cursor <= body.span.end) {
                        continue;
                    }

                    if let Some((array, receiver)) = find_validate_array(body, cursor) {
                        let fields = harvest_fields(&body.stmts, &receiver);
                        if let Some(action) = build_action(source, sv, &array, &fields, uri) {
                            out.push(action);
                        }
                    }

                    if m.name == "rules"
                        && let Some(array) = find_rules_return_array(body, cursor)
                    {
                        let fields = harvest_fields_across_class(c, &Receiver::ThisKeyword);
                        if let Some(action) = build_action(source, sv, &array, &fields, uri) {
                            out.push(action);
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect(&inner.stmts, source, sv, cursor, uri, out);
                }
            }
            _ => {}
        }
    }
}

struct FoundArray {
    span: Span,
    existing_keys: Vec<String>,
}

fn to_found_array(
    elements: &php_ast::ArenaVec<'_, ArrayElement<'_, '_>>,
    span: Span,
) -> FoundArray {
    let existing_keys = elements
        .iter()
        .filter_map(|el| match &el.key {
            Some(Expr {
                kind: ExprKind::String(s),
                ..
            }) => Some((*s).to_string()),
            _ => None,
        })
        .collect();
    FoundArray {
        span,
        existing_keys,
    }
}

/// Finds a `receiver->validate([...])` call (receiver: `$request` or
/// `$this`) whose array-literal argument's span contains `cursor`.
fn find_validate_array(
    body: &php_ast::Block<'_, '_>,
    cursor: u32,
) -> Option<(FoundArray, Receiver)> {
    struct Finder {
        cursor: u32,
        found: Option<(FoundArray, Receiver)>,
    }
    impl<'arena, 'src> Visitor<'arena, 'src> for Finder {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if self.found.is_some() {
                return ControlFlow::Break(());
            }
            if let ExprKind::MethodCall(mc) = &expr.kind
                && let ExprKind::Identifier(method) = &mc.method.kind
                && method.eq_ignore_ascii_case("validate")
                && let Some(arg) = mc.args.first()
                && let ExprKind::Array(elements) = &arg.value.kind
                && arg.value.span.start <= self.cursor
                && self.cursor <= arg.value.span.end
            {
                let receiver = match &mc.object.kind {
                    ExprKind::Variable(v) if v.as_str() == "this" => Some(Receiver::ThisKeyword),
                    ExprKind::Variable(v) => Some(Receiver::Variable(v.as_str().to_string())),
                    _ => None,
                };
                if let Some(receiver) = receiver {
                    self.found = Some((to_found_array(elements, arg.value.span), receiver));
                    return ControlFlow::Break(());
                }
            }
            walk_expr(self, expr)
        }
    }
    let mut finder = Finder {
        cursor,
        found: None,
    };
    for stmt in body.stmts.iter() {
        if matches!(finder.visit_stmt(stmt), ControlFlow::Break(())) {
            break;
        }
    }
    finder.found
}

/// Finds a `return [...]` statement whose array literal's span contains
/// `cursor`, anywhere in `body` (including nested blocks).
fn find_rules_return_array(body: &php_ast::Block<'_, '_>, cursor: u32) -> Option<FoundArray> {
    fn walk<'a>(stmts: &[Stmt<'a, 'a>], cursor: u32) -> Option<FoundArray> {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Return(Some(expr)) => {
                    if let ExprKind::Array(elements) = &expr.kind
                        && expr.span.start <= cursor
                        && cursor <= expr.span.end
                    {
                        return Some(to_found_array(elements, expr.span));
                    }
                }
                StmtKind::Block(inner) => {
                    if let Some(found) = walk(&inner.stmts, cursor) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
    walk(&body.stmts, cursor)
}

/// Harvests `receiver->input()`/etc. fields across every method in `c`, not
/// just one — a FormRequest's `rules()` body has nothing to harvest from
/// itself, so this looks at the whole class.
fn harvest_fields_across_class(c: &php_ast::ClassDecl<'_, '_>, receiver: &Receiver) -> Vec<String> {
    let mut out = Vec::new();
    for member in c.body.members.iter() {
        if let ClassMemberKind::Method(m) = &member.kind
            && let Some(body) = &m.body
        {
            for field in harvest_fields(&body.stmts, receiver) {
                if !out.contains(&field) {
                    out.push(field);
                }
            }
        }
    }
    out
}

fn build_action(
    source: &str,
    sv: SourceView<'_>,
    array: &FoundArray,
    fields: &[String],
    uri: &Uri,
) -> Option<CodeActionOrCommand> {
    let missing: Vec<&String> = fields
        .iter()
        .filter(|f| !array.existing_keys.iter().any(|k| k == *f))
        .collect();
    if missing.is_empty() {
        return None;
    }

    // Insert right after the array's opening `[` — always syntactically
    // valid (PHP allows a trailing comma before `]`) regardless of whether
    // the array is empty or already has entries.
    let open_bracket = source[array.span.start as usize..array.span.end as usize]
        .find('[')
        .map(|i| array.span.start + i as u32 + 1)?;
    let pos = sv.position_of(open_bracket);

    let new_text: String = missing
        .iter()
        .map(|f| format!("'{f}' => 'required', "))
        .collect();

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text,
        }],
    );

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Generate validation rules from $request usages".to_string(),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}
