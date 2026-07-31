/// Code action: "Extract method" — moves selected statements inside a class method
/// into a new `private function extractedMethod()` on the same class.
///
/// Variable analysis:
/// - Variables that appear in the selection **and** were assigned/used before the
///   selection starts become **parameters** of the extracted method (`mixed $x`).
/// - Variables that are **assigned inside** the selection and referenced **after**
///   the selection ends become the **return value** (single variable for now).
use std::collections::HashMap;

use php_ast::{ClassMemberKind, NamespaceBody, StmtKind};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView};
use crate::text::{selected_text_range, utf16_offset_to_byte};

/// Return a "Extract method" code action when `range` spans multiple lines inside
/// a class method body. Returns an empty vec when the preconditions are not met.
pub fn extract_method_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    if range.start.line >= range.end.line {
        return vec![];
    }

    let sv = doc.view();
    let stmts = &doc.program().stmts;
    let (class_end_offset, method_is_static, enclosing_params) =
        match find_enclosing_class(stmts, sv, range) {
            Some(info) => info,
            None => return vec![],
        };

    let selected = selected_text_range(source, range);
    if selected.trim().is_empty() {
        return vec![];
    }

    let before = text_before(source, range);
    let after = text_after(source, range);

    // A variable is already bound before the selection either because it was
    // assigned there, or because it's a parameter of the enclosing method —
    // `collect_assigned_vars` only sees `$x = ...` text and has no notion of
    // the method signature, so parameters must be added explicitly.
    let mut vars_before = collect_assigned_vars(&before);
    for p in &enclosing_params {
        if !vars_before.contains(p) {
            vars_before.push(p.clone());
        }
    }
    let vars_in_selection = collect_vars_in_text(&selected);
    let params: Vec<String> = vars_in_selection
        .iter()
        .filter(|v| vars_before.contains(v))
        .cloned()
        .collect();

    let vars_assigned_in = collect_assigned_vars(&selected);
    let vars_used_after = collect_vars_in_text(&after);
    let returned: Option<String> = vars_assigned_in
        .into_iter()
        .find(|v| vars_used_after.contains(v));

    let indent = line_indent(source, range.start.line);
    let call_prefix = if method_is_static {
        "self::"
    } else {
        "$this->"
    };
    let params_call_list = params.join(", ");
    let call_text = match &returned {
        Some(ret_var) => {
            format!("{indent}{ret_var} = {call_prefix}extractedMethod({params_call_list});\n")
        }
        None => format!("{indent}{call_prefix}extractedMethod({params_call_list});\n"),
    };

    let static_kw = if method_is_static { "static " } else { "" };
    let param_decls: String = params
        .iter()
        .map(|v| format!("mixed {v}"))
        .collect::<Vec<_>>()
        .join(", ");
    let return_type = match &returned {
        Some(_) => ": mixed",
        None => ": void",
    };
    let method_body = selected.trim_end_matches('\n').to_string();

    let return_stmt = match &returned {
        Some(ret_var) => format!("\n        return {ret_var};"),
        None => String::new(),
    };

    let new_method = format!(
        "\n    private {static_kw}function extractedMethod({param_decls}){return_type}\n    {{\n{body}{return_stmt}\n    }}\n",
        body = indent_block(&method_body, "        "),
    );

    let closing_line = sv.position_of(class_end_offset.saturating_sub(1)).line;
    let insert_pos = Position {
        line: closing_line,
        character: 0,
    };

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![
            TextEdit {
                range,
                new_text: call_text,
            },
            TextEdit {
                range: Range {
                    start: insert_pos,
                    end: insert_pos,
                },
                new_text: new_method,
            },
        ],
    );

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: "Extract method".to_string(),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

/// Returns `(class_span_end_offset, method_is_static, enclosing_method_params)`
/// when `range` is inside a class method body, walking into namespaced blocks
/// as needed. `enclosing_method_params` holds the method's own parameter
/// names (e.g. `"$name"`) — these are bound before the selection even though
/// they're never assigned via `$x = ...`.
fn find_enclosing_class(
    stmts: &[php_ast::Stmt<'_, '_>],
    sv: SourceView<'_>,
    range: Range,
) -> Option<(u32, bool, Vec<String>)> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let class_start = sv.position_of(stmt.span.start).line;
                let class_end = sv.position_of(stmt.span.end).line;
                if range.start.line < class_start || range.end.line > class_end {
                    continue;
                }
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let method_start = sv.position_of(member.span.start).line;
                        let method_end = sv.position_of(member.span.end).line;
                        if range.start.line >= method_start && range.end.line <= method_end {
                            let params = m.params.iter().map(|p| format!("${}", p.name)).collect();
                            return Some((stmt.span.end, m.is_static, params));
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_enclosing_class(&inner.stmts, sv, range)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

/// Collect every `$varName` (excluding `$this`) appearing anywhere in `text`.
fn collect_vars_in_text(text: &str) -> Vec<String> {
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
                if name != "this" && !vars.contains(&full) {
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

/// Collect variable names that appear on the left-hand side of a simple assignment
/// (`$var =`) in `text`.  This is a heuristic text scan; it handles the common
/// cases (`$x = …`, `$x +=`, etc.) without a full parse.
fn collect_assigned_vars(text: &str) -> Vec<String> {
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
                let mut j = end;
                while j < bytes.len() && bytes[j] == b' ' {
                    j += 1;
                }
                // `=` but not `==` or `===`
                let is_assignment = j < bytes.len()
                    && bytes[j] == b'='
                    && (j + 1 >= bytes.len() || bytes[j + 1] != b'=');
                let is_compound = j + 1 < bytes.len()
                    && (bytes[j] == b'+'
                        || bytes[j] == b'-'
                        || bytes[j] == b'*'
                        || bytes[j] == b'/'
                        || bytes[j] == b'.')
                    && bytes[j + 1] == b'=';
                if is_assignment || is_compound {
                    let name = &text[start..end];
                    let full = format!("${name}");
                    if name != "this" && !vars.contains(&full) {
                        vars.push(full);
                    }
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    vars
}

/// Return the source text that comes before `range`.
fn text_before(source: &str, range: Range) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut result = String::new();
    for (i, line) in lines.iter().enumerate() {
        let i = i as u32;
        if i < range.start.line {
            result.push_str(line);
            result.push('\n');
        } else if i == range.start.line {
            let end = utf16_offset_to_byte(line, range.start.character as usize);
            result.push_str(&line[..end]);
            break;
        } else {
            break;
        }
    }
    result
}

/// Return the source text that comes after `range`.
fn text_after(source: &str, range: Range) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut result = String::new();
    for (i, line) in lines.iter().enumerate() {
        let i = i as u32;
        if i > range.end.line {
            result.push_str(line);
            result.push('\n');
        } else if i == range.end.line {
            let start = utf16_offset_to_byte(line, range.end.character as usize);
            result.push_str(&line[start..]);
            result.push('\n');
        }
    }
    result
}

/// Return the leading whitespace of line `line` in `source`.
fn line_indent(source: &str, line: u32) -> String {
    source
        .lines()
        .nth(line as usize)
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_default()
}

/// Re-indent a block of text so every non-empty line starts with `prefix`.
fn indent_block(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else {
                format!("{prefix}{}", line.trim_start())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
