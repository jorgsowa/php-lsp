use tower_lsp::lsp_types::{FormattingOptions, Position, Range, TextEdit};

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
/// opening brace and copies its line's indentation.
fn close_brace(source: &str, position: Position) -> Vec<TextEdit> {
    let lines: Vec<&str> = source.lines().collect();
    let cur_idx = position.line as usize;
    let cur_line = match lines.get(cur_idx) {
        Some(l) => *l,
        None => return vec![],
    };
    let cur_indent = leading_whitespace(cur_line);

    // Backward scan: depth=1 because we're looking for the `{` that opened
    // the block the just-typed `}` closes.
    let mut depth: i32 = 1;
    let mut match_indent: Option<&str> = None;

    'outer: for i in (0..cur_idx).rev() {
        for ch in lines[i].chars().rev() {
            match ch {
                '}' => depth += 1,
                '{' => {
                    depth -= 1;
                    if depth == 0 {
                        match_indent = Some(leading_whitespace(lines[i]));
                        break 'outer;
                    }
                }
                _ => {}
            }
        }
    }

    let new_indent = match_indent.unwrap_or("");

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
fn indent_new_line(
    source: &str,
    position: Position,
    options: &FormattingOptions,
) -> Vec<TextEdit> {
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

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(spaces: bool, size: u32) -> FormattingOptions {
        FormattingOptions {
            tab_size: size,
            insert_spaces: spaces,
            ..Default::default()
        }
    }

    fn pos(line: u32, ch: u32) -> Position {
        Position { line, character: ch }
    }

    // ── \n tests ─────────────────────────────────────────────────────────────

    #[test]
    fn new_line_after_open_brace_indents() {
        // Source after Enter pressed: line 2 is new empty line
        let src = "<?php\nif (true) {\n\n}";
        let edits = on_type_format(src, pos(2, 0), "\n", &opts(true, 4));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "    ");
    }

    #[test]
    fn new_line_copies_base_indent() {
        let src = "<?php\n    $x = 1;\n";
        let edits = on_type_format(src, pos(2, 0), "\n", &opts(true, 4));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "    ");
    }

    #[test]
    fn new_line_uses_tabs_when_configured() {
        let src = "<?php\nif (true) {\n\n}";
        let edits = on_type_format(src, pos(2, 0), "\n", &opts(false, 4));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "\t");
    }

    #[test]
    fn new_line_at_top_level_no_edit() {
        let src = "<?php\n";
        let edits = on_type_format(src, pos(0, 0), "\n", &opts(true, 4));
        assert!(edits.is_empty());
    }

    #[test]
    fn new_line_no_edit_when_already_correct() {
        let src = "<?php\nif (true) {\n    ";
        let edits = on_type_format(src, pos(2, 0), "\n", &opts(true, 4));
        assert!(edits.is_empty(), "no edit needed when indent already correct");
    }

    // ── } tests ──────────────────────────────────────────────────────────────

    #[test]
    fn close_brace_aligns_to_opening() {
        let src = "<?php\nif (true) {\n    $x = 1;\n    }";
        let edits = on_type_format(src, pos(3, 4), "}", &opts(true, 4));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, ""); // opening `{` is at column 0
    }

    #[test]
    fn close_brace_already_aligned_no_edit() {
        let src = "<?php\nif (true) {\n    $x = 1;\n}";
        let edits = on_type_format(src, pos(3, 0), "}", &opts(true, 4));
        assert!(edits.is_empty());
    }

    #[test]
    fn close_brace_nested_block() {
        let src = "<?php\nclass Foo {\n    public function bar() {\n        $x = 1;\n        }\n}";
        //                                                                              ^ we're formatting this `}`
        let edits = on_type_format(src, pos(4, 8), "}", &opts(true, 4));
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "    "); // matches `public function bar() {` indentation
    }

    #[test]
    fn unknown_trigger_returns_no_edits() {
        let src = "<?php\n$x = 1;";
        let edits = on_type_format(src, pos(1, 8), ";", &opts(true, 4));
        assert!(edits.is_empty());
    }
}
