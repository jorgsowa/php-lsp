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
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ast::{ParsedDoc, SourceView};
use crate::util::utf16_offset_to_byte;

// ── Public entry point ────────────────────────────────────────────────────────

/// Return a "Extract method" code action when `range` spans multiple lines inside
/// a class method body. Returns an empty vec when the preconditions are not met.
pub fn extract_method_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    // Only trigger on multi-line selections.
    if range.start.line >= range.end.line {
        return vec![];
    }

    // Find the enclosing class and method.
    let sv = doc.view();
    let stmts = &doc.program().stmts;
    let (class_end_offset, method_is_static) = match find_enclosing_class(stmts, sv, range) {
        Some(info) => info,
        None => return vec![],
    };

    let selected = selected_text(source, range);
    if selected.trim().is_empty() {
        return vec![];
    }

    // Split the source at the selection boundaries so we can compare variable
    // usage in each region.
    let before = text_before(source, range);
    let after = text_after(source, range);

    // Variables that appear before the selection and also inside it → parameters.
    let vars_before = collect_assigned_vars(&before);
    let vars_in_selection = collect_vars_in_text(&selected);
    let params: Vec<String> = vars_in_selection
        .iter()
        .filter(|v| vars_before.contains(v))
        .cloned()
        .collect();

    // Variables assigned inside the selection that are also used after it → return value.
    let vars_assigned_in = collect_assigned_vars(&selected);
    let vars_used_after = collect_vars_in_text(&after);
    let returned: Option<String> = vars_assigned_in
        .into_iter()
        .find(|v| vars_used_after.contains(v));

    // ── Build the replacement call ────────────────────────────────────────────
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

    // ── Build the new method ──────────────────────────────────────────────────
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

    // Insert the new method just before the closing brace of the class.
    let closing_line = sv.position_of(class_end_offset.saturating_sub(1)).line;
    let insert_pos = Position {
        line: closing_line,
        character: 0,
    };

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![
            // Replace the selected lines with the method call.
            TextEdit {
                range,
                new_text: call_text,
            },
            // Insert the extracted method before the class closing brace.
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

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `(class_span_end_offset, method_is_static)` when `range` is inside a
/// class method body, walking into namespaced blocks as needed.
fn find_enclosing_class(
    stmts: &[php_ast::Stmt<'_, '_>],
    sv: SourceView<'_>,
    range: Range,
) -> Option<(u32, bool)> {
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
                            return Some((stmt.span.end, m.is_static));
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
                // Skip whitespace after the variable name.
                let mut j = end;
                while j < bytes.len() && bytes[j] == b' ' {
                    j += 1;
                }
                // Check for assignment operator (=, +=, -=, *=, /=, .=, etc.)
                // but NOT == or ===.
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

/// Return the selected text defined by `range`.
fn selected_text(source: &str, range: Range) -> String {
    let lines: Vec<&str> = source.lines().collect();
    if range.start.line == range.end.line {
        let line = match lines.get(range.start.line as usize) {
            Some(l) => l,
            None => return String::new(),
        };
        let start = utf16_offset_to_byte(line, range.start.character as usize);
        let end = utf16_offset_to_byte(line, range.end.character as usize);
        line[start..end].to_string()
    } else {
        let mut result = String::new();
        for i in range.start.line..=range.end.line {
            let line = match lines.get(i as usize) {
                Some(l) => *l,
                None => break,
            };
            if i == range.start.line {
                let start = utf16_offset_to_byte(line, range.start.character as usize);
                result.push_str(&line[start..]);
            } else if i == range.end.line {
                let end = utf16_offset_to_byte(line, range.end.character as usize);
                result.push_str(&line[..end]);
            } else {
                result.push_str(line);
            }
            if i < range.end.line {
                result.push('\n');
            }
        }
        result
    }
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
