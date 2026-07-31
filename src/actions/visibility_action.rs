use std::collections::HashMap;

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind, Visibility};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView};

/// Offer "Make public / protected / private" code actions when the cursor is
/// on a method, property, or class-constant declaration inside a class or trait.
///
/// Returns at most two actions (the two non-current visibility levels).
/// No action is offered when the cursor is inside a method body rather than
/// on the declaration line itself.
pub fn change_visibility_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let cursor_byte = sv.byte_of_position(range.start) as usize;
    let mut out = Vec::new();
    collect(&doc.program().stmts, source, cursor_byte, uri, sv, &mut out);
    out
}

fn collect<'a>(
    stmts: &[Stmt<'a, 'a>],
    source: &str,
    cursor_byte: usize,
    uri: &Uri,
    sv: SourceView<'_>,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(cls) => {
                visit_members(&cls.body.members, source, cursor_byte, uri, sv, out);
            }
            StmtKind::Trait(t) => {
                visit_members(&t.body.members, source, cursor_byte, uri, sv, out);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect(&inner.stmts, source, cursor_byte, uri, sv, out);
                }
            }
            _ => {}
        }
    }
}

fn visit_members<'a>(
    members: &[php_ast::ClassMember<'a, 'a>],
    source: &str,
    cursor_byte: usize,
    uri: &Uri,
    sv: SourceView<'_>,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for member in members {
        let span_start = member.span.start as usize;
        let span_end = member.span.end as usize;
        if cursor_byte < span_start || cursor_byte >= span_end {
            continue;
        }
        let vis_and_attrs = match &member.kind {
            ClassMemberKind::Method(m) => m.visibility.map(|v| (v, &m.attributes)),
            ClassMemberKind::Property(p) => p.visibility.map(|v| (v, &p.attributes)),
            ClassMemberKind::ClassConst(c) => c.visibility.map(|v| (v, &c.attributes)),
            ClassMemberKind::TraitUse(_) => None,
        };
        if let Some((vis, attrs)) = vis_and_attrs {
            // Skip past any leading attributes before searching for the
            // visibility keyword — an attribute argument string containing
            // "private"/"public"/"protected" (e.g. an Assert\Choice list)
            // would otherwise be matched instead of the real modifier.
            let search_start = attrs
                .last()
                .map(|a| a.span.end as usize)
                .unwrap_or(span_start);
            push_actions(source, uri, sv, search_start, cursor_byte, vis, out);
        }
        break;
    }
}

fn push_actions(
    source: &str,
    uri: &Uri,
    sv: SourceView<'_>,
    span_start: usize,
    cursor_byte: usize,
    current: Visibility,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let Some(kw_range) = find_visibility_range(source, span_start, current) else {
        return;
    };

    // Only offer when cursor is on the declaration line (not inside the method body).
    let kw_line = sv.position_of(kw_range.start as u32).line;
    let cursor_line = sv.position_of(cursor_byte as u32).line;
    if kw_line != cursor_line {
        return;
    }

    let candidates: &[Visibility] = match current {
        Visibility::Public => &[Visibility::Protected, Visibility::Private],
        Visibility::Protected => &[Visibility::Public, Visibility::Private],
        Visibility::Private => &[Visibility::Public, Visibility::Protected],
    };

    for &new_vis in candidates {
        let new_kw = vis_str(new_vis);
        let edit_range = Range {
            start: sv.position_of(kw_range.start as u32),
            end: sv.position_of(kw_range.end as u32),
        };
        let mut changes = HashMap::new();
        changes.insert(
            uri.clone(),
            vec![TextEdit {
                range: edit_range,
                new_text: new_kw.to_string(),
            }],
        );
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Make {new_kw}"),
            kind: Some(CodeActionKind::REFACTOR),
            edit: Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
            ..Default::default()
        }));
    }
}

/// Find the byte range of the visibility keyword within a member's span.
/// `span_start` is expected to already be past any leading attributes (see
/// the caller); searches up to 256 bytes from there to accommodate other
/// leading modifiers (`final`, `static`, `readonly`, …) before the keyword.
fn find_visibility_range(
    source: &str,
    span_start: usize,
    vis: Visibility,
) -> Option<std::ops::Range<usize>> {
    let keyword = vis_str(vis);
    let end = (span_start + 256).min(source.len());
    let window = &source[span_start..end];

    let mut search_offset = 0;
    while let Some(idx) = window[search_offset..].find(keyword) {
        let idx_in_window = search_offset + idx;
        let abs = span_start + idx_in_window;
        let end_abs = abs + keyword.len();

        let before_ok = idx_in_window == 0
            || !source[..abs]
                .chars()
                .next_back()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        let after_ok = end_abs >= source.len()
            || !source[end_abs..]
                .chars()
                .next()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);

        if before_ok && after_ok {
            return Some(abs..end_abs);
        }
        search_offset = idx_in_window + 1;
    }
    None
}

fn vis_str(vis: Visibility) -> &'static str {
    match vis {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}
