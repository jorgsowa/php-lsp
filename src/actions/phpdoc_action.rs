/// Code action: generate a PHPDoc stub for a function or method that lacks one.
use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Param, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ast::{ParsedDoc, SourceView, format_type_hint};

/// Return "Generate PHPDoc" code actions for any function/method whose declaration line
/// falls within `range` and does not already have a docblock.
pub fn phpdoc_actions(
    uri: &Url,
    doc: &ParsedDoc,
    _source: &str,
    range: Range,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let mut actions = Vec::new();
    collect(&doc.program().stmts, uri, sv, range, &mut actions);
    actions
}

fn collect(
    stmts: &[Stmt<'_, '_>],
    uri: &Url,
    sv: SourceView<'_>,
    range: Range,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let fn_line = sv.position_of(stmt.span.start).line;
                if line_in_range(fn_line, range) && f.doc_comment.is_none() {
                    let ret = f.return_type.as_ref().map(|t| format_type_hint(t));
                    if let Some(action) = make_action(uri, sv.source(), fn_line, &f.params, ret) {
                        out.push(action);
                    }
                }
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let fn_line = sv.position_of(member.span.start).line;
                        if line_in_range(fn_line, range) && m.doc_comment.is_none() {
                            let ret = m.return_type.as_ref().map(|t| format_type_hint(t));
                            if let Some(action) =
                                make_action(uri, sv.source(), fn_line, &m.params, ret)
                            {
                                out.push(action);
                            }
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let fn_line = sv.position_of(member.span.start).line;
                        if line_in_range(fn_line, range) && m.doc_comment.is_none() {
                            let ret = m.return_type.as_ref().map(|t| format_type_hint(t));
                            if let Some(action) =
                                make_action(uri, sv.source(), fn_line, &m.params, ret)
                            {
                                out.push(action);
                            }
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        let fn_line = sv.position_of(member.span.start).line;
                        if line_in_range(fn_line, range) && m.doc_comment.is_none() {
                            let ret = m.return_type.as_ref().map(|t| format_type_hint(t));
                            if let Some(action) =
                                make_action(uri, sv.source(), fn_line, &m.params, ret)
                            {
                                out.push(action);
                            }
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect(&inner.stmts, uri, sv, range, out);
                }
            }
            _ => {}
        }
    }
}

fn line_in_range(line: u32, range: Range) -> bool {
    line >= range.start.line && line <= range.end.line
}

fn make_action(
    uri: &Url,
    source: &str,
    fn_line: u32,
    params: &[Param<'_, '_>],
    return_type: Option<String>,
) -> Option<CodeActionOrCommand> {
    let indent = source
        .lines()
        .nth(fn_line as usize)
        .map(|line| {
            let n = line.len() - line.trim_start().len();
            &line[..n]
        })
        .unwrap_or("")
        .to_string();

    let mut lines: Vec<String> = vec![format!("{indent}/**")];

    for p in params.iter() {
        let type_part = p
            .type_hint
            .as_ref()
            .map(|t| format!("{} ", format_type_hint(t)))
            .unwrap_or_default();
        let name = format!("${}", p.name);
        lines.push(format!("{indent} * @param {type_part}{name}"));
    }

    if let Some(ret) = return_type {
        lines.push(format!("{indent} * @return {ret}"));
    }

    lines.push(format!("{indent} */"));

    let new_text = lines.join("\n") + "\n";
    let pos = Position {
        line: fn_line,
        character: 0,
    };
    let edit = TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text,
    };

    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), vec![edit]);

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Generate PHPDoc".to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}
