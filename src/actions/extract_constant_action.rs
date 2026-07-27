/// Code action: "Extract constant" — extracts a selected literal into a named PHP constant.
use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::text::selected_text_range;

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
    // A class/interface constant initializer must be a compile-time constant
    // expression — a double-quoted string with variable interpolation
    // (`"Hello $name"`, `"{$this->x}"`) is not one, and hoisting it verbatim
    // would be a PHP fatal error ("Constant expression contains invalid
    // operations"). Single-quoted strings never interpolate, so they're safe
    // regardless of content.
    if trimmed.starts_with('"') && has_double_quoted_interpolation(&trimmed[1..trimmed.len() - 1])
    {
        return vec![];
    }

    let const_name = derive_const_name(trimmed);
    let lines: Vec<&str> = source.lines().collect();
    let sel_line = range.start.line as usize;

    // Selecting the RHS of an existing `const NAME = <literal>;` would insert a
    // second declaration of that name above it — a PHP fatal error ("Cannot
    // redefine class constant"). Bail out rather than offer a broken action.
    if lines
        .get(sel_line)
        .is_some_and(|line| is_const_declaration(line))
    {
        return vec![];
    }

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

fn is_const_declaration(line: &str) -> bool {
    line.trim()
        .trim_start_matches("public ")
        .trim_start_matches("private ")
        .trim_start_matches("protected ")
        .trim_start_matches("final ")
        .starts_with("const ")
}

fn is_literal(s: &str) -> bool {
    is_string_literal(s) || is_int_literal(s) || is_float_literal(s)
}

fn is_string_literal(s: &str) -> bool {
    (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
}

/// True if `inner` (a double-quoted string's content, without the quotes)
/// contains a construct PHP treats as variable interpolation: `$name`,
/// `${name}`, or `{$expr}`. A lone `$` not followed by an identifier start
/// (e.g. `"price: $5"`) does not interpolate and is left alone.
fn has_double_quoted_interpolation(inner: &str) -> bool {
    let bytes = inner.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2, // escaped char — skip both bytes, `\$` doesn't interpolate
            b'$' => {
                let starts_var = bytes
                    .get(i + 1)
                    .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_' || *b == b'{');
                if starts_var {
                    return true;
                }
                i += 1;
            }
            b'{' if bytes.get(i + 1) == Some(&b'$') => return true,
            _ => i += 1,
        }
    }
    false
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
            for (j, brace_line) in lines.iter().enumerate().skip(i) {
                if brace_line.contains('{') {
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
///
/// Skips `{`/`}` inside strings and comments so that `"hello { world }"` or
/// `// }` does not prematurely close the depth counter.
fn find_matching_close(lines: &[&str], open_line: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_block_comment = false;
    for (i, line) in lines.iter().enumerate().skip(open_line) {
        let bytes = line.as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            if in_block_comment {
                if j + 1 < bytes.len() && bytes[j] == b'*' && bytes[j + 1] == b'/' {
                    in_block_comment = false;
                    j += 2;
                } else {
                    j += 1;
                }
                continue;
            }
            match bytes[j] {
                b'"' | b'\'' => {
                    let quote = bytes[j];
                    j += 1;
                    while j < bytes.len() {
                        if bytes[j] == b'\\' {
                            j += 2; // skip escaped char (ASCII-safe: non-first UTF-8 bytes cannot be b'\\')
                        } else if bytes[j] == quote {
                            j += 1;
                            break;
                        } else {
                            j += 1;
                        }
                    }
                }
                b'/' if j + 1 < bytes.len() && bytes[j + 1] == b'/' => break, // // comment
                b'#' => break,                                                // # comment
                b'/' if j + 1 < bytes.len() && bytes[j + 1] == b'*' => {
                    in_block_comment = true;
                    j += 2;
                }
                b'{' => {
                    depth += 1;
                    j += 1;
                }
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                    j += 1;
                }
                _ => j += 1,
            }
        }
    }
    None
}

fn container_kind(line: &str) -> Option<ContainerKind> {
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
