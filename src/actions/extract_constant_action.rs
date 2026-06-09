/// Code action: "Extract constant" — extracts a selected literal into a named PHP constant.
use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::util::selected_text_range;

/// When the selection is a string, integer, or float literal, offer to extract
/// it into a named constant.
///
/// - Inside a `class` or `trait`: inserts `private const NAME = value;` and
///   replaces the selection with `self::NAME`.
/// - Inside an `interface`: inserts `const NAME = value;` (interface constants
///   are implicitly public; `private` is invalid there).
/// - At file scope: inserts `const NAME = value;` and replaces with `NAME`.
///
/// The constant name is derived from the literal value (SCREAMING_SNAKE_CASE
/// for strings, `CONSTANT_<value>` for numbers). Use the LSP rename action to
/// pick a more meaningful name.
pub fn extract_constant_actions(source: &str, range: Range, uri: &Url) -> Vec<CodeActionOrCommand> {
    if range.start == range.end {
        return vec![];
    }

    let selected = selected_text_range(source, range);
    let trimmed = selected.trim();
    if trimmed.is_empty() || !is_literal(trimmed) {
        return vec![];
    }

    let const_name = derive_const_name(trimmed);
    let lines: Vec<&str> = source.lines().collect();
    let sel_line = range.start.line as usize;

    match find_class_scope(&lines, sel_line) {
        Some((insert_line, kind)) => {
            let insert_pos = Position {
                line: insert_line as u32 + 1,
                character: 0,
            };
            let decl = match kind {
                ContainerKind::Interface => format!("    const {const_name} = {trimmed};\n"),
                ContainerKind::ClassOrTrait => {
                    format!("    private const {const_name} = {trimmed};\n")
                }
            };
            let reference = format!("self::{const_name}");
            build_action("Extract constant", decl, insert_pos, reference, range, uri)
        }
        None => {
            let insert_line = file_scope_insert_line(&lines);
            let insert_pos = Position {
                line: insert_line as u32,
                character: 0,
            };
            let decl = format!("const {const_name} = {trimmed};\n");
            build_action("Extract constant", decl, insert_pos, const_name, range, uri)
        }
    }
}

// ── Literal detection ─────────────────────────────────────────────────────────

fn is_literal(s: &str) -> bool {
    is_string_literal(s) || is_int_literal(s) || is_float_literal(s)
}

fn is_string_literal(s: &str) -> bool {
    (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
}

fn is_int_literal(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn is_float_literal(s: &str) -> bool {
    let mut dots = 0u32;
    !s.is_empty()
        && s.chars().all(|c| {
            if c == '.' {
                dots += 1;
                dots == 1
            } else {
                c.is_ascii_digit()
            }
        })
        && dots == 1
}

// ── Constant name derivation ──────────────────────────────────────────────────

fn derive_const_name(literal: &str) -> String {
    if is_string_literal(literal) {
        let inner = &literal[1..literal.len() - 1];
        derive_name_from_string(inner)
    } else {
        let sanitised = literal.replace('.', "_");
        format!("CONSTANT_{sanitised}")
    }
}

fn derive_name_from_string(s: &str) -> String {
    let raw: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .to_uppercase();

    // Collapse consecutive underscores, strip leading and trailing underscores.
    let mut name = String::new();
    let mut prev_under = true;
    for c in raw.chars() {
        if c == '_' {
            if !prev_under {
                name.push('_');
            }
            prev_under = true;
        } else {
            name.push(c);
            prev_under = false;
        }
    }
    let name = name.trim_end_matches('_').to_string();

    // PHP identifiers cannot start with a digit.
    let name = if name.starts_with(|c: char| c.is_ascii_digit()) {
        format!("CONSTANT_{name}")
    } else {
        name
    };

    if name.is_empty() {
        "EXTRACTED_CONSTANT".to_string()
    } else {
        name
    }
}

// ── Scope detection ───────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum ContainerKind {
    ClassOrTrait,
    Interface,
}

/// Scan backwards from `sel_line` to find an enclosing class, interface, or
/// trait declaration.  Returns `(brace_line, kind)` where `brace_line` is the
/// 0-based index of the line containing the opening `{`.
///
/// The selection must be strictly inside the container body (between the
/// opening `{` and its matching `}`).
fn find_class_scope(lines: &[&str], sel_line: usize) -> Option<(usize, ContainerKind)> {
    for i in (0..=sel_line).rev() {
        let line = lines[i].trim();
        if let Some(kind) = container_kind(line) {
            // Find the opening brace.
            for (j, brace_line) in lines.iter().enumerate().skip(i) {
                if brace_line.contains('{') {
                    // Verify the selection falls inside the container body.
                    if find_matching_close(lines, j)
                        .is_some_and(|close| sel_line > j && sel_line < close)
                    {
                        return Some((j, kind));
                    }
                    break;
                }
            }
        }
    }
    None
}

/// Starting at `open_line` (which contains the opening `{`), scan forward and
/// return the 0-based line index of the matching closing `}`.
fn find_matching_close(lines: &[&str], open_line: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, line) in lines.iter().enumerate().skip(open_line) {
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// Returns `Some(ContainerKind)` if `line` is a class/interface/trait declaration.
fn container_kind(line: &str) -> Option<ContainerKind> {
    // Strip PHP modifier keywords before the type keyword.
    let stripped = line
        .trim_start_matches("abstract ")
        .trim_start_matches("final ")
        .trim_start_matches("readonly ");
    if stripped.starts_with("class ")
        || stripped.starts_with("class{")
        || stripped.starts_with("trait ")
        || stripped.starts_with("trait{")
    {
        Some(ContainerKind::ClassOrTrait)
    } else if stripped.starts_with("interface ") || stripped.starts_with("interface{") {
        Some(ContainerKind::Interface)
    } else {
        None
    }
}

/// Find the first line after `<?php`, blank lines, `namespace`, and `use`
/// statements.  The new `const` declaration will be inserted before that line.
///
/// Scanning stops at the first non-preamble line to prevent the insertion point
/// from jumping past code that already exists in the file.
fn file_scope_insert_line(lines: &[&str]) -> usize {
    let mut last_preamble = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("<?php")
            || t.is_empty()
            || t.starts_with("namespace ")
            || t.starts_with("use ")
        {
            last_preamble = i + 1;
        } else {
            break;
        }
    }
    last_preamble
}

// ── Action builder ────────────────────────────────────────────────────────────

fn build_action(
    title: &str,
    decl: String,
    insert_pos: Position,
    reference: String,
    replace_range: Range,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![
            TextEdit {
                range: Range {
                    start: insert_pos,
                    end: insert_pos,
                },
                new_text: decl,
            },
            TextEdit {
                range: replace_range,
                new_text: reference,
            },
        ],
    );
    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_name_from_url_string() {
        assert_eq!(
            derive_name_from_string("https://api.example.com"),
            "HTTPS_API_EXAMPLE_COM"
        );
    }

    #[test]
    fn derive_name_empty_string_fallback() {
        assert_eq!(derive_name_from_string("!!!"), "EXTRACTED_CONSTANT");
    }

    #[test]
    fn derive_name_leading_digit_prefixed() {
        assert_eq!(derive_name_from_string("42abc"), "CONSTANT_42ABC");
    }
}
