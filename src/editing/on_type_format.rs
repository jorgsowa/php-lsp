use tower_lsp::lsp_types::{FormattingOptions, Position, Range, TextEdit};

use super::signature_help::string_literal_mask;

/// Compute formatting edits triggered by typing a single character.
///
/// Supported trigger characters:
/// - `}` — de-indent to align with the matching `{`
/// - `\n` — indent the new line based on the previous line's context
pub fn on_type_format(
    source: &str,
    position: Position,
    ch: &str,
    options: &FormattingOptions,
) -> Vec<TextEdit> {
    match ch {
        "}" => close_brace(source, position),
        "\n" => indent_new_line(source, position, options),
        _ => vec![],
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn leading_whitespace(line: &str) -> &str {
    let trimmed = line.trim_start();
    &line[..line.len() - trimmed.len()]
}

fn indent_unit(options: &FormattingOptions) -> String {
    if options.insert_spaces {
        " ".repeat(options.tab_size as usize)
    } else {
        "\t".to_string()
    }
}

// ── `}` handler ──────────────────────────────────────────────────────────────

/// De-indent the line containing `}` to match its corresponding `{`.
///
/// Scans backward through the source, tracking brace depth, to find the
/// opening brace and copies its line's indentation. Braces inside string
/// literals or comments (e.g. a property default of `"}"`) are skipped via
/// `string_literal_mask`, so they can't be mistaken for real block delimiters.
fn close_brace(source: &str, position: Position) -> Vec<TextEdit> {
    let lines: Vec<&str> = source.lines().collect();
    let cur_idx = position.line as usize;
    let cur_line = match lines.get(cur_idx) {
        Some(l) => *l,
        None => return vec![],
    };
    let cur_indent = leading_whitespace(cur_line);

    let chars: Vec<char> = source.chars().collect();
    let mask = string_literal_mask(&chars);

    // Global char index of the start of `cur_idx`'s line.
    let mut line_start = chars.len();
    let mut line_no = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        if line_no == cur_idx {
            line_start = i;
            break;
        }
        if c == '\n' {
            line_no += 1;
        }
    }

    // Backward scan: depth=1 because we're looking for the `{` that opened
    // the block the just-typed `}` closes.
    let mut depth: i32 = 1;
    let mut match_line: Option<usize> = None;
    let mut scan_line = cur_idx;
    let mut idx = line_start;
    while idx > 0 {
        idx -= 1;
        let c = chars[idx];
        if c == '\n' {
            scan_line -= 1;
            continue;
        }
        if mask[idx] {
            continue;
        }
        match c {
            '}' => depth += 1,
            '{' => {
                depth -= 1;
                if depth == 0 {
                    match_line = Some(scan_line);
                    break;
                }
            }
            _ => {}
        }
    }

    let new_indent = match_line.and_then(|l| lines.get(l)).map_or("", |l| leading_whitespace(l));

    if new_indent == cur_indent {
        return vec![];
    }

    vec![TextEdit {
        range: Range {
            start: Position {
                line: position.line,
                character: 0,
            },
            end: Position {
                line: position.line,
                character: cur_indent.len() as u32,
            },
        },
        new_text: new_indent.to_string(),
    }]
}

// ── `\n` handler ─────────────────────────────────────────────────────────────

/// Indent the new line after Enter is pressed.
///
/// - Copies the previous (non-empty) line's indentation as a base.
/// - Adds one extra indent level when the previous line ends with `{`.
fn indent_new_line(source: &str, position: Position, options: &FormattingOptions) -> Vec<TextEdit> {
    let lines: Vec<&str> = source.lines().collect();
    let new_idx = position.line as usize;

    if new_idx == 0 {
        return vec![];
    }

    // Previous non-empty line
    let prev = (0..new_idx)
        .rev()
        .find_map(|i| {
            let l = *lines.get(i)?;
            if !l.trim().is_empty() { Some(l) } else { None }
        })
        .unwrap_or("");

    let base_indent = leading_whitespace(prev);
    let desired = if prev.trim_end().ends_with('{') {
        format!("{}{}", base_indent, indent_unit(options))
    } else {
        base_indent.to_string()
    };

    if desired.is_empty() {
        return vec![];
    }

    // Replace whatever whitespace the editor already put on the new line
    let curr = lines.get(new_idx).copied().unwrap_or("");
    let curr_ws = leading_whitespace(curr);

    if desired == curr_ws {
        return vec![];
    }

    vec![TextEdit {
        range: Range {
            start: Position {
                line: position.line,
                character: 0,
            },
            end: Position {
                line: position.line,
                character: curr_ws.len() as u32,
            },
        },
        new_text: desired,
    }]
}
