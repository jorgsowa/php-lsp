/// Code action: "Inline variable" — replaces all usages of a variable with its
/// initializer expression and removes the assignment line.
///
/// Only acts when:
/// - The cursor/selection is on or inside a variable name (e.g. `$extracted`).
/// - There is exactly one visible assignment `$var = <expr>;` on a single line
///   earlier in the same scope.
/// - The RHS is a single-line expression (multi-line RHS is not supported).
use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::util::word_at_position;

pub fn inline_variable_actions(source: &str, range: Range, uri: &Url) -> Vec<CodeActionOrCommand> {
    // Determine the variable name under cursor (or at start of selection).
    let cursor = range.start;
    let var_name = match word_at_position(source, cursor) {
        Some(w) if w.starts_with('$') => w,
        _ => return vec![],
    };

    // Require exactly one visible assignment in the file. Multiple writes
    // make inlining ambiguous (which RHS?) and unsafe (we'd silently drop
    // one), so we refuse rather than guess.
    let (assign_line_no, rhs) = match find_unique_assignment(source, &var_name, cursor.line) {
        Some(v) => v,
        None => return vec![],
    };

    // Collect all usages of `$var` in the source below the assignment line.
    let usages = collect_usages(source, &var_name, assign_line_no + 1);
    if usages.is_empty() {
        return vec![];
    }

    // Build edits: replace each usage with the RHS, then delete the assignment line.
    let mut edits: Vec<TextEdit> = usages
        .into_iter()
        .map(|usage_range| TextEdit {
            range: usage_range,
            new_text: rhs.clone(),
        })
        .collect();

    // Delete the assignment line (including its newline).
    edits.push(TextEdit {
        range: Range {
            start: Position {
                line: assign_line_no,
                character: 0,
            },
            end: Position {
                line: assign_line_no + 1,
                character: 0,
            },
        },
        new_text: String::new(),
    });

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Inline variable '{var_name}'"),
        kind: Some(CodeActionKind::REFACTOR_INLINE),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

/// Find the single `$var = <expr>;` assignment in `source`. Returns
/// `(line_number, rhs_text)` only if exactly one such line exists *and* it
/// appears before `before_line` — any second write, before or after the
/// cursor, disqualifies the inline. Compound assignments (`+=`, `-=`, …) and
/// equality (`==`) are ignored.
fn find_unique_assignment(source: &str, var_name: &str, before_line: u32) -> Option<(u32, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let mut hit: Option<(u32, String)> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let prefix = format!("{var_name} =");
        let Some(rest) = trimmed.strip_prefix(prefix.as_str()) else {
            continue;
        };
        // Reject `$var ==` (equality) — `strip_prefix("$var =")` matches both.
        if rest.starts_with('=') {
            continue;
        }
        let rhs = rest.trim().trim_end_matches(';').trim();
        if rhs.is_empty() {
            continue;
        }
        if hit.is_some() {
            return None; // more than one write → ambiguous
        }
        hit = Some((i as u32, rhs.to_string()));
    }

    // The unique assignment must precede the cursor; otherwise usage collection
    // (which only scans *below* the assignment) would miss the cursor's usage.
    hit.filter(|(line_no, _)| *line_no < before_line)
}

/// Find all occurrences of `$var` in `source` at or after `from_line`.
/// Returns LSP `Range`s covering each occurrence.
fn collect_usages(source: &str, var_name: &str, from_line: u32) -> Vec<Range> {
    let mut usages = Vec::new();
    for (line_idx, line) in source.lines().enumerate() {
        if (line_idx as u32) < from_line {
            continue;
        }
        let mut search_from = 0usize;
        while let Some(pos) = line[search_from..].find(var_name) {
            let abs = search_from + pos;
            // Word-boundary check: character before must not be alphanumeric/$/_
            let before_ok = abs == 0
                || line
                    .as_bytes()
                    .get(abs - 1)
                    .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');
            // Character after must not be alphanumeric/_
            let after_ok = line
                .as_bytes()
                .get(abs + var_name.len())
                .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');

            if before_ok && after_ok {
                // Skip if this looks like an assignment target: `$var =`
                let after_var = line[abs + var_name.len()..].trim_start();
                if after_var.starts_with('=') && !after_var.starts_with("==") {
                    search_from = abs + var_name.len();
                    continue;
                }

                let char_start = byte_col_to_utf16_col(line, abs);
                let char_end = byte_col_to_utf16_col(line, abs + var_name.len());
                usages.push(Range {
                    start: Position {
                        line: line_idx as u32,
                        character: char_start as u32,
                    },
                    end: Position {
                        line: line_idx as u32,
                        character: char_end as u32,
                    },
                });
            }
            search_from = abs + 1;
        }
    }
    usages
}

fn byte_col_to_utf16_col(line: &str, byte_col: usize) -> usize {
    line[..byte_col.min(line.len())]
        .chars()
        .map(|c| c.len_utf16())
        .sum()
}
