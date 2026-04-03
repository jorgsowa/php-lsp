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

use crate::util::word_at;

pub fn inline_variable_actions(source: &str, range: Range, uri: &Url) -> Vec<CodeActionOrCommand> {
    // Determine the variable name under cursor (or at start of selection).
    let cursor = range.start;
    let var_name = match word_at(source, cursor) {
        Some(w) if w.starts_with('$') => w,
        _ => return vec![],
    };

    // Find the single-line assignment above the cursor.
    let (assign_line_no, rhs) = match find_assignment(source, &var_name, cursor.line) {
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

/// Scan backward from `before_line` (exclusive) to find the nearest
/// `$var = <expr>;` assignment.  Returns `(line_number, rhs_text)`.
fn find_assignment(source: &str, var_name: &str, before_line: u32) -> Option<(u32, String)> {
    let lines: Vec<&str> = source.lines().collect();
    let search_up_to = (before_line as usize).min(lines.len());

    for i in (0..search_up_to).rev() {
        let line = lines[i].trim();
        // Match `$var = <expr>;`
        let prefix = format!("{var_name} =");
        if let Some(rest) = line.strip_prefix(prefix.as_str()) {
            let rhs = rest.trim().trim_end_matches(';').trim();
            if !rhs.is_empty() {
                return Some((i as u32, rhs.to_string()));
            }
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    #[test]
    fn no_action_when_cursor_not_on_variable() {
        let src = "<?php\n$x = 1;\nfoo();\n";
        let range = Range {
            start: Position {
                line: 2,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 0,
            },
        };
        let actions = inline_variable_actions(src, range, &uri());
        assert!(actions.is_empty(), "should not act on non-variable cursor");
    }

    #[test]
    fn no_action_when_no_assignment_found() {
        let src = "<?php\necho $x;\n";
        let range = Range {
            start: Position {
                line: 1,
                character: 5,
            },
            end: Position {
                line: 1,
                character: 7,
            },
        };
        let actions = inline_variable_actions(src, range, &uri());
        assert!(actions.is_empty(), "no assignment to inline");
    }

    #[test]
    fn inlines_single_usage() {
        let src = "<?php\n$x = new Foo();\necho $x;\n";
        let range = Range {
            start: Position {
                line: 2,
                character: 5,
            },
            end: Position {
                line: 2,
                character: 7,
            },
        };
        let actions = inline_variable_actions(src, range, &uri());
        assert!(!actions.is_empty(), "should produce an action");
        let CodeActionOrCommand::CodeAction(ca) = &actions[0] else {
            panic!("expected CodeAction");
        };
        let edits = ca
            .edit
            .as_ref()
            .unwrap()
            .changes
            .as_ref()
            .unwrap()
            .values()
            .next()
            .unwrap();
        // One replacement edit + one deletion edit
        assert_eq!(edits.len(), 2, "expected replacement + deletion edits");
        // The replacement should be the RHS
        let replacement = edits.iter().find(|e| e.new_text == "new Foo()");
        assert!(
            replacement.is_some(),
            "replacement should use RHS 'new Foo()'"
        );
    }

    #[test]
    fn action_kind_is_refactor_inline() {
        let src = "<?php\n$val = 42;\nreturn $val;\n";
        let range = Range {
            start: Position {
                line: 2,
                character: 7,
            },
            end: Position {
                line: 2,
                character: 11,
            },
        };
        let CodeActionOrCommand::CodeAction(ca) = &inline_variable_actions(src, range, &uri())[0]
        else {
            panic!();
        };
        assert_eq!(ca.kind, Some(CodeActionKind::REFACTOR_INLINE));
    }
}
