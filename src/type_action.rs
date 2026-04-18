/// Code action: "Add return type declaration" for functions/methods that lack one.
use std::collections::HashMap;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ast::{ParsedDoc, SourceView};

/// Return "Add return type" code actions for any function/method within `range`
/// that has no return type annotation and a concrete body.
pub fn add_return_type_actions(
    _source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Url,
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
    uri: &Url,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let fn_line = sv.position_of(stmt.span.start).line;
                if line_in_range(fn_line, range) && f.return_type.is_none() {
                    let returns_value = body_has_value_return(&f.body);
                    let type_str = if returns_value { "mixed" } else { "void" };
                    if let Some(insert) =
                        find_close_paren_offset(sv.source(), stmt.span.start as usize)
                    {
                        push_action(sv, insert, type_str, uri, out);
                    }
                }
                // Recurse into nested functions
                collect_in_stmts(&f.body, sv, range, uri, out);
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
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
                            let type_str = if body_has_value_return(body) {
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
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && let fn_line = sv.position_of(member.span.start).line
                        && line_in_range(fn_line, range)
                        && m.return_type.is_none()
                        && let Some(body) = &m.body
                        && let Some(insert) =
                            find_close_paren_offset(sv.source(), member.span.start as usize)
                    {
                        let type_str = if body_has_value_return(body) {
                            "mixed"
                        } else {
                            "void"
                        };
                        push_action(sv, insert, type_str, uri, out);
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && let fn_line = sv.position_of(member.span.start).line
                        && line_in_range(fn_line, range)
                        && m.return_type.is_none()
                        && let Some(body) = &m.body
                        && let Some(insert) =
                            find_close_paren_offset(sv.source(), member.span.start as usize)
                    {
                        let type_str = if body_has_value_return(body) {
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
                    collect(inner, sv, range, uri, out);
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
    uri: &Url,
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
            body_has_value_return(&t.body)
                || t.catches.iter().any(|c| body_has_value_return(&c.body))
                || t.finally
                    .as_ref()
                    .map(|f| body_has_value_return(f))
                    .unwrap_or(false)
        }
        StmtKind::Block(inner) => body_has_value_return(inner),
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
    uri: &Url,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    fn full_range() -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: u32::MAX,
                character: u32::MAX,
            },
        }
    }

    fn point(line: u32) -> Range {
        Range {
            start: Position { line, character: 0 },
            end: Position { line, character: 0 },
        }
    }

    fn first_edit_text(actions: &[CodeActionOrCommand]) -> String {
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let changes = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            changes.values().next().unwrap()[0].new_text.clone()
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn offers_void_for_function_with_no_return() {
        let src = "<?php\nfunction greet() {}";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(1), &uri());
        assert_eq!(actions.len(), 1);
        assert_eq!(first_edit_text(&actions), ": void");
    }

    #[test]
    fn offers_mixed_for_function_with_value_return() {
        let src = "<?php\nfunction getId() { return 42; }";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(1), &uri());
        assert_eq!(actions.len(), 1);
        assert_eq!(first_edit_text(&actions), ": mixed");
    }

    #[test]
    fn no_action_when_return_type_exists() {
        let src = "<?php\nfunction getId(): int { return 42; }";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(1), &uri());
        assert!(
            actions.is_empty(),
            "should not offer action when return type is already present"
        );
    }

    #[test]
    fn no_action_when_cursor_not_on_function() {
        let src = "<?php\nfunction greet() {}";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(5), &uri());
        assert!(actions.is_empty());
    }

    #[test]
    fn void_inserted_after_close_paren() {
        let src = "<?php\nfunction greet() {}";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(1), &uri());
        assert_eq!(actions.len(), 1);
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let changes = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edit = &changes.values().next().unwrap()[0];
            // Insertion point should be right after `)`
            // "function greet()" — `)` is at column 15 (0-indexed), insert at 16
            assert_eq!(edit.range.start.line, 1);
            assert_eq!(edit.range.start.character, 16);
            assert_eq!(edit.range.start, edit.range.end, "must be a pure insertion");
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn offers_void_for_method_with_no_return() {
        let src = "<?php\nclass Foo {\n    public function bar() {}\n}";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(2), &uri());
        assert_eq!(actions.len(), 1);
        assert_eq!(first_edit_text(&actions), ": void");
    }

    #[test]
    fn offers_mixed_for_method_with_value_return() {
        let src = "<?php\nclass Foo {\n    public function getId() { return $this->id; }\n}";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(2), &uri());
        assert_eq!(actions.len(), 1);
        assert_eq!(first_edit_text(&actions), ": mixed");
    }

    #[test]
    fn skips_constructor() {
        let src = "<?php\nclass Foo {\n    public function __construct() {}\n}";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, full_range(), &uri());
        assert!(
            actions.is_empty(),
            "should not offer return type for __construct"
        );
    }

    #[test]
    fn void_for_function_returning_void_explicitly() {
        let src = "<?php\nfunction run() { return; }";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(1), &uri());
        assert_eq!(actions.len(), 1);
        assert_eq!(first_edit_text(&actions), ": void");
    }

    #[test]
    fn mixed_for_if_return_in_method() {
        let src = "<?php\nclass Foo {\n    public function get() { if (true) { return 1; } }\n}";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(2), &uri());
        assert_eq!(actions.len(), 1);
        assert_eq!(first_edit_text(&actions), ": mixed");
    }

    #[test]
    fn action_title_contains_type() {
        let src = "<?php\nfunction greet() {}";
        let d = doc(src);
        let actions = add_return_type_actions(src, &d, point(1), &uri());
        assert_eq!(actions.len(), 1);
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            assert!(
                a.title.contains("void"),
                "title should mention the type, got: {}",
                a.title
            );
        }
    }
}
