/// Code action: "Add return type declaration" for functions/methods that lack one.
use std::collections::HashMap;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView};

/// Return "Add return type" code actions for any function/method within `range`
/// that has no return type annotation and a concrete body.
pub fn add_return_type_actions(
    _source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let mut out = Vec::new();
    collect(&doc.program().stmts, sv, range, uri, &mut out);
    out
}

fn collect(
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    range: Range,
    uri: &Uri,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let fn_line = sv.position_of(stmt.span.start).line;
                if line_in_range(fn_line, range) && f.return_type.is_none() {
                    let returns_value = body_has_value_return(&f.body.stmts);
                    let type_str = if returns_value { "mixed" } else { "void" };
                    if let Some(insert) =
                        find_close_paren_offset(sv.source(), stmt.span.start as usize)
                    {
                        push_action(sv, insert, type_str, uri, out);
                    }
                }
                // Recurse into nested functions
                collect_in_stmts(&f.body.stmts, sv, range, uri, out);
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == "__construct" {
                            continue;
                        }
                        let fn_line = sv.position_of(member.span.start).line;
                        if line_in_range(fn_line, range)
                            && m.return_type.is_none()
                            && let Some(body) = &m.body
                            && let Some(insert) =
                                find_close_paren_offset(sv.source(), member.span.start as usize)
                        {
                            let type_str = if body_has_value_return(&body.stmts) {
                                "mixed"
                            } else {
                                "void"
                            };
                            push_action(sv, insert, type_str, uri, out);
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && let fn_line = sv.position_of(member.span.start).line
                        && line_in_range(fn_line, range)
                        && m.return_type.is_none()
                        && let Some(body) = &m.body
                        && let Some(insert) =
                            find_close_paren_offset(sv.source(), member.span.start as usize)
                    {
                        let type_str = if body_has_value_return(&body.stmts) {
                            "mixed"
                        } else {
                            "void"
                        };
                        push_action(sv, insert, type_str, uri, out);
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && let fn_line = sv.position_of(member.span.start).line
                        && line_in_range(fn_line, range)
                        && m.return_type.is_none()
                        && let Some(body) = &m.body
                        && let Some(insert) =
                            find_close_paren_offset(sv.source(), member.span.start as usize)
                    {
                        let type_str = if body_has_value_return(&body.stmts) {
                            "mixed"
                        } else {
                            "void"
                        };
                        push_action(sv, insert, type_str, uri, out);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect(&inner.stmts, sv, range, uri, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_in_stmts(
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    range: Range,
    uri: &Uri,
    out: &mut Vec<CodeActionOrCommand>,
) {
    collect(stmts, sv, range, uri, out);
}

fn line_in_range(line: u32, range: Range) -> bool {
    line >= range.start.line && line <= range.end.line
}

/// Returns `true` if any `return <expr>` (non-void) statement appears
/// directly inside `stmts` (does not recurse into nested functions/closures).
fn body_has_value_return(stmts: &[Stmt<'_, '_>]) -> bool {
    stmts.iter().any(|s| stmt_has_value_return(s))
}

fn stmt_has_value_return(stmt: &Stmt<'_, '_>) -> bool {
    match &stmt.kind {
        StmtKind::Return(Some(_)) => true,
        // Do not recurse into nested function/closure bodies.
        StmtKind::Function(_) => false,
        StmtKind::Class(_) | StmtKind::Trait(_) | StmtKind::Enum(_) => false,
        StmtKind::If(i) => {
            stmt_has_value_return(i.then_branch)
                || i.elseif_branches
                    .iter()
                    .any(|ei| stmt_has_value_return(&ei.body))
                || i.else_branch
                    .as_ref()
                    .map(|e| stmt_has_value_return(e))
                    .unwrap_or(false)
        }
        StmtKind::While(w) => stmt_has_value_return(w.body),
        StmtKind::For(f) => stmt_has_value_return(f.body),
        StmtKind::Foreach(f) => stmt_has_value_return(f.body),
        StmtKind::DoWhile(d) => stmt_has_value_return(d.body),
        StmtKind::TryCatch(t) => {
            body_has_value_return(&t.body.stmts)
                || t.catches
                    .iter()
                    .any(|c| body_has_value_return(&c.body.stmts))
                || t.finally
                    .as_ref()
                    .map(|f| body_has_value_return(&f.stmts))
                    .unwrap_or(false)
        }
        StmtKind::Block(inner) => body_has_value_return(&inner.stmts),
        _ => false,
    }
}

/// Scan `sv.source()` starting at `from` (byte offset) and return the byte offset
/// immediately after the `)` that closes the first `(...)` group encountered.
/// Skips single- and double-quoted string literals.
fn find_close_paren_offset(source: &str, from: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth: i32 = 0;
    let mut i = from;

    while i < bytes.len() {
        match bytes[i] {
            b'\'' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'\'' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                continue;
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'\\' => i += 2,
                        b'"' => {
                            i += 1;
                            break;
                        }
                        _ => i += 1,
                    }
                }
                continue;
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn push_action(
    sv: SourceView<'_>,
    after_close_paren: usize,
    type_str: &str,
    uri: &Uri,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let pos = sv.position_of(after_close_paren as u32);
    let insert_pos = Position {
        line: pos.line,
        character: pos.character,
    };
    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: insert_pos,
                end: insert_pos,
            },
            new_text: format!(": {type_str}"),
        }],
    );
    out.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Add return type `: {type_str}`"),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }));
}
