/// Code action: "Convert facade call to dependency injection" — rewrites a
/// `Cache::get(...)`-style static facade call under the cursor, inside a
/// non-static instance method, to `$this->cache->get(...)`, adding a
/// constructor-promoted parameter for the facade's contract interface
/// (generating a constructor if the class doesn't have one yet).
///
/// v1 scope: rewrites only the call site under the cursor, not every
/// occurrence of the facade in the class — same single-site scope every
/// other action in this module has (extract, inline, promote, …).
use std::collections::HashMap;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{ClassMemberKind, Expr, ExprKind, NamespaceBody, Span, Stmt, StmtKind};
use std::ops::ControlFlow;
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView};
use crate::laravel::facades;

pub fn facade_to_di_actions(
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
                    if m.is_static {
                        continue;
                    }
                    let Some(body) = &m.body else { continue };
                    if !(body.span.start <= cursor && cursor <= body.span.end) {
                        continue;
                    }
                    let Some(call) = find_facade_call(body, cursor) else {
                        continue;
                    };
                    if has_member_named(c, call.prop) {
                        continue;
                    }
                    out.push(build_action(source, sv, c, &call, uri));
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

struct FacadeCall {
    /// Span of the class-name-and-`::`, e.g. `Cache::` in `Cache::get(...)`.
    prefix_span: Span,
    facade: String,
    contract: &'static str,
    prop: &'static str,
}

fn find_facade_call(body: &php_ast::Block<'_, '_>, cursor: u32) -> Option<FacadeCall> {
    struct Finder {
        cursor: u32,
        found: Option<FacadeCall>,
    }
    impl<'arena, 'src> Visitor<'arena, 'src> for Finder {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if self.found.is_some() {
                return ControlFlow::Break(());
            }
            if let ExprKind::StaticMethodCall(s) = &expr.kind
                && expr.span.start <= self.cursor
                && self.cursor <= expr.span.end
                && let ExprKind::Identifier(class_ident) = &s.class.kind
                && let Some((contract, prop)) = facades::lookup(class_ident.as_str())
            {
                self.found = Some(FacadeCall {
                    prefix_span: Span::new(s.class.span.start, s.method.span.start),
                    facade: class_ident.as_str().to_string(),
                    contract,
                    prop,
                });
                return ControlFlow::Break(());
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

/// Whether `prop_name` collides with an existing property (including
/// constructor-promoted params) — the class already has something to
/// inject into, so don't offer a second, conflicting injection.
fn has_member_named(c: &php_ast::ClassDecl<'_, '_>, prop_name: &str) -> bool {
    for member in c.body.members.iter() {
        match &member.kind {
            ClassMemberKind::Property(p) if p.name == prop_name => return true,
            ClassMemberKind::Method(m)
                if m.name == "__construct" && m.params.iter().any(|p| p.name == prop_name) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

fn build_action(
    source: &str,
    sv: SourceView<'_>,
    c: &php_ast::ClassDecl<'_, '_>,
    call: &FacadeCall,
    uri: &Uri,
) -> CodeActionOrCommand {
    let mut edits = Vec::new();

    // Rewrite the call prefix: `Cache::` → `$this->cache->`.
    edits.push(TextEdit {
        range: sv.range_of(call.prefix_span),
        new_text: format!("$this->{}->", call.prop),
    });

    let ctor = c.body.members.iter().find_map(|m| {
        if let ClassMemberKind::Method(method) = &m.kind
            && method.name == "__construct"
        {
            Some((m, method))
        } else {
            None
        }
    });

    match ctor {
        Some((member, method)) => match method.params.last() {
            Some(last_param) => {
                let pos = sv.position_of(last_param.span.end);
                edits.push(TextEdit {
                    range: Range {
                        start: pos,
                        end: pos,
                    },
                    new_text: format!(", private \\{} ${}", call.contract, call.prop),
                });
            }
            None => {
                // Empty param list — no `Param` span to anchor on, so scan
                // forward from the member's own span for the opening `(`.
                let paren =
                    first_paren_after(source, member.span.start).unwrap_or(member.span.start);
                let pos = sv.position_of(paren);
                edits.push(TextEdit {
                    range: Range {
                        start: pos,
                        end: pos,
                    },
                    new_text: format!("private \\{} ${}", call.contract, call.prop),
                });
            }
        },
        None => {
            let closing_line = sv.position_of(c.body.span.end.saturating_sub(1)).line;
            let pos = tower_lsp_server::ls_types::Position {
                line: closing_line,
                character: 0,
            };
            edits.push(TextEdit {
                range: Range {
                    start: pos,
                    end: pos,
                },
                new_text: format!(
                    "    public function __construct(private \\{} ${})\n    {{\n    }}\n\n",
                    call.contract, call.prop
                ),
            });
        }
    }

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);

    CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Convert {}:: call to dependency injection", call.facade),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })
}

fn first_paren_after(source: &str, from_offset: u32) -> Option<u32> {
    source[from_offset as usize..]
        .find('(')
        .map(|i| from_offset + i as u32 + 1)
}
