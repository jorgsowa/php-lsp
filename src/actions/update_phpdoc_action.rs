/// Code action: update an existing PHPDoc to match the current function/method
/// signature (add missing @param, remove stale @param, rename mismatched @param,
/// add missing @return).
use std::collections::HashMap;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Param, Stmt, StmtKind, TypeHint};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView, format_type_hint};
use crate::lang::docblock::{Docblock, parse_docblock};

/// Return "Update PHPDoc to match signature" for every function/method whose
/// declaration line falls within `range`, already has a docblock, and whose
/// @param/@return section is out of sync with the actual signature.
pub fn update_phpdoc_actions(uri: &Uri, doc: &ParsedDoc, range: Range) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let mut out = Vec::new();
    collect_stmts(&doc.program().stmts, uri, sv, range, &mut out);
    out
}

fn collect_stmts<'a>(
    stmts: &[Stmt<'a, 'a>],
    uri: &Uri,
    sv: SourceView<'_>,
    range: Range,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                if line_in_range(sv.position_of(stmt.span.start).line, range)
                    && let Some(dc) = &f.doc_comment
                {
                    maybe_push(uri, sv, dc, &f.params, f.return_type.as_ref(), out);
                }
            }
            StmtKind::Class(c) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && line_in_range(sv.position_of(member.span.start).line, range)
                        && let Some(dc) = &m.doc_comment
                    {
                        maybe_push(uri, sv, dc, &m.params, m.return_type.as_ref(), out);
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && line_in_range(sv.position_of(member.span.start).line, range)
                        && let Some(dc) = &m.doc_comment
                    {
                        maybe_push(uri, sv, dc, &m.params, m.return_type.as_ref(), out);
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && line_in_range(sv.position_of(member.span.start).line, range)
                        && let Some(dc) = &m.doc_comment
                    {
                        maybe_push(uri, sv, dc, &m.params, m.return_type.as_ref(), out);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_stmts(&inner.stmts, uri, sv, range, out);
                }
            }
            _ => {}
        }
    }
}

fn maybe_push<'a>(
    uri: &Uri,
    sv: SourceView<'_>,
    doc_comment: &php_ast::Comment<'a>,
    params: &[Param<'a, 'a>],
    return_type: Option<&TypeHint<'a, 'a>>,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let parsed = parse_docblock(doc_comment.text);

    if !needs_update(&parsed, params, return_type) {
        return;
    }

    let source = sv.source();
    let doc_start_line = sv.position_of(doc_comment.span.start).line;
    let doc_end_line = sv.position_of(doc_comment.span.end.saturating_sub(1)).line;

    let indent = extract_indent(source, doc_start_line);
    let new_text = generate_updated_docblock(&indent, &parsed, params, return_type);

    let edit_range = Range {
        start: Position {
            line: doc_start_line,
            character: 0,
        },
        end: Position {
            line: doc_end_line + 1,
            character: 0,
        },
    };

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: edit_range,
            new_text,
        }],
    );

    out.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Update PHPDoc to match signature".to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }));
}

/// Return `true` when the docblock's @param/@return section is out of sync.
///
/// Triggers when param count or names differ, or when the signature declares a
/// return type but the docblock has no @return tag. Type-hint changes for
/// already-matching params are not compared (the generated block always uses
/// the signature type, so they are updated unconditionally on the next trigger).
fn needs_update<'a>(
    parsed: &Docblock,
    params: &[Param<'a, 'a>],
    return_type: Option<&TypeHint<'a, 'a>>,
) -> bool {
    if parsed.is_inherit_doc {
        return false;
    }
    if parsed.params.len() != params.len() {
        return true;
    }
    for (doc_p, actual_p) in parsed.params.iter().zip(params.iter()) {
        let doc_name = doc_p.name.trim_start_matches('$');
        let actual_name = actual_p.name.or_error();
        if doc_name != actual_name {
            return true;
        }
    }
    // Missing @return when the signature declares a non-void return type.
    // void is self-documenting via the type annotation; don't force @return void.
    if let Some(ret) = return_type
        && format_type_hint(ret) != "void"
        && parsed.return_type.is_none()
    {
        return true;
    }
    false
}

fn extract_indent(source: &str, line: u32) -> String {
    source
        .lines()
        .nth(line as usize)
        .map(|l| {
            let n = l.len() - l.trim_start().len();
            l[..n].to_string()
        })
        .unwrap_or_default()
}

fn generate_updated_docblock<'a>(
    indent: &str,
    parsed: &Docblock,
    params: &[Param<'a, 'a>],
    return_type: Option<&TypeHint<'a, 'a>>,
) -> String {
    let mut lines: Vec<String> = vec![format!("{indent}/**")];

    // Description — preserve verbatim, one `* ` line per input line.
    if !parsed.description.is_empty() {
        for desc_line in parsed.description.lines() {
            let trimmed = desc_line.trim();
            if trimmed.is_empty() {
                lines.push(format!("{indent} *"));
            } else {
                lines.push(format!("{indent} * {trimmed}"));
            }
        }
        if !params.is_empty() || return_type.is_some() || parsed.return_type.is_some() {
            lines.push(format!("{indent} *"));
        }
    }

    // @param — one per actual parameter.
    for param in params {
        let actual_name = param.name.or_error();
        let type_hint = param
            .type_hint
            .as_ref()
            .map(|t| format_type_hint(t))
            .or_else(|| {
                // Fall back to the existing docblock type for the same param name.
                parsed
                    .params
                    .iter()
                    .find(|p| p.name.trim_start_matches('$') == actual_name)
                    .filter(|p| !p.type_hint.is_empty() && p.type_hint != "mixed")
                    .map(|p| p.type_hint.clone())
            })
            .unwrap_or_else(|| "mixed".to_string());

        let desc = parsed
            .params
            .iter()
            .find(|p| p.name.trim_start_matches('$') == actual_name)
            .map(|p| p.description.as_str())
            .unwrap_or("");

        if desc.is_empty() {
            lines.push(format!("{indent} * @param {type_hint} ${actual_name}"));
        } else {
            lines.push(format!(
                "{indent} * @param {type_hint} ${actual_name} {desc}"
            ));
        }
    }

    // @return — use signature type if present and non-void; otherwise preserve existing.
    // void is self-documenting; don't clutter the docblock with @return void.
    if let Some(ret) = return_type {
        let ret_type = format_type_hint(ret);
        if ret_type != "void" {
            let desc = parsed
                .return_type
                .as_ref()
                .map(|r| r.description.as_str())
                .unwrap_or("");
            if desc.is_empty() {
                lines.push(format!("{indent} * @return {ret_type}"));
            } else {
                lines.push(format!("{indent} * @return {ret_type} {desc}"));
            }
        }
    } else if let Some(doc_ret) = &parsed.return_type {
        let desc = doc_ret.description.as_str();
        if desc.is_empty() {
            lines.push(format!("{indent} * @return {}", doc_ret.type_hint));
        } else {
            lines.push(format!("{indent} * @return {} {desc}", doc_ret.type_hint));
        }
    }

    // @throws — preserved from existing docblock.
    for t in &parsed.throws {
        if t.description.is_empty() {
            lines.push(format!("{indent} * @throws {}", t.class));
        } else {
            lines.push(format!("{indent} * @throws {} {}", t.class, t.description));
        }
    }

    // @see / @link — preserved.
    for s in &parsed.see {
        lines.push(format!("{indent} * @see {s}"));
    }

    lines.push(format!("{indent} */"));
    lines.join("\n") + "\n"
}

fn line_in_range(line: u32, range: Range) -> bool {
    line >= range.start.line && line <= range.end.line
}
