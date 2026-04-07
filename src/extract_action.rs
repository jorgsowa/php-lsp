/// Code action: "Extract variable" — wraps the selected expression in a `$extracted` variable.
use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::util::utf16_offset_to_byte;

/// When the selection is non-empty and appears to be an expression, offer to
/// extract it into a local variable.  The generated variable name is `$extracted`
/// (a safe, unambiguous placeholder that the user can then rename with the LSP
/// rename action).
pub fn extract_variable_actions(source: &str, range: Range, uri: &Url) -> Vec<CodeActionOrCommand> {
    // Only act on non-empty selections.
    if range.start == range.end {
        return vec![];
    }
    let selected = selected_text(source, range);
    if selected.is_empty() || selected.trim().is_empty() {
        return vec![];
    }
    // Don't offer on selections that are already a simple variable or plain keyword.
    let trimmed = selected.trim();
    if trimmed.starts_with('$')
        && trimmed
            .chars()
            .skip(1)
            .all(|c| c.is_alphanumeric() || c == '_')
    {
        return vec![];
    }

    // Determine the indentation of the line where the selection starts.
    let indent = line_indent(source, range.start.line);

    // Insert `$extracted = <expression>;` on the line before the start of the selection.
    let insert_pos = Position {
        line: range.start.line,
        character: 0,
    };
    let insert_text = format!("{indent}$extracted = {trimmed};\n");

    // Replace the selected text with `$extracted`.
    let replace_text = "$extracted".to_string();

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![
            TextEdit {
                range: Range {
                    start: insert_pos,
                    end: insert_pos,
                },
                new_text: insert_text,
            },
            TextEdit {
                range,
                new_text: replace_text,
            },
        ],
    );

    vec![CodeActionOrCommand::CodeAction(CodeAction {
        title: "Extract variable".to_string(),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })]
}

// ── Helpers ───────────────────────────────────────────────────────────────────

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

fn line_indent(source: &str, line: u32) -> String {
    source
        .lines()
        .nth(line as usize)
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).collect())
        .unwrap_or_default()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
        Range {
            start: Position {
                line: sl,
                character: sc,
            },
            end: Position {
                line: el,
                character: ec,
            },
        }
    }

    #[test]
    fn empty_selection_produces_no_action() {
        let src = "<?php\n$x = foo();";
        let r = range(1, 4, 1, 4);
        let actions = extract_variable_actions(src, r, &uri());
        assert!(
            actions.is_empty(),
            "empty selection should not produce actions"
        );
    }

    #[test]
    fn simple_variable_selection_produces_no_action() {
        let src = "<?php\n$x = $foo;";
        // Select "$foo"
        let r = range(1, 4, 1, 8);
        let actions = extract_variable_actions(src, r, &uri());
        assert!(
            actions.is_empty(),
            "selecting a simple variable should not produce extract action"
        );
    }

    #[test]
    fn expression_selection_produces_extract_action() {
        let src = "<?php\n$x = foo() + bar();";
        // "$x = foo() + bar();"  -- "foo() + bar()" is col 5..18
        let r = range(1, 5, 1, 18);
        let actions = extract_variable_actions(src, r, &uri());
        assert!(!actions.is_empty(), "expected extract variable action");
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            assert_eq!(a.title, "Extract variable");
            let edits = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let texts: Vec<&str> = edits
                .values()
                .next()
                .unwrap()
                .iter()
                .map(|e| e.new_text.as_str())
                .collect();
            // One edit inserts the assignment
            assert!(
                texts
                    .iter()
                    .any(|t| t.contains("$extracted = foo() + bar();")),
                "should insert assignment"
            );
            // Another edit replaces with $extracted
            assert!(
                texts.iter().any(|&t| t == "$extracted"),
                "should replace with $extracted"
            );
        }
    }

    #[test]
    fn extract_preserves_indentation() {
        let src = "<?php\nfunction foo() {\n    $x = bar();\n}";
        // Select "bar()" on line 2 col 9..14
        let r = range(2, 9, 2, 14);
        let actions = extract_variable_actions(src, r, &uri());
        assert!(!actions.is_empty());
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let edits = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let insert = edits
                .values()
                .next()
                .unwrap()
                .iter()
                .find(|e| e.new_text.contains("$extracted ="))
                .unwrap();
            assert!(
                insert.new_text.starts_with("    "),
                "should preserve indentation"
            );
        }
    }

    #[test]
    fn selected_text_correct_with_multibyte_prefix() {
        // Line: $x = "café";
        //        0123456789...
        // "é" (U+00E9) is 2 bytes in UTF-8, 1 code unit in UTF-16.
        // Selecting "café" starts at UTF-16 char 5 (after `$x = "`) and ends at 9.
        // In raw bytes those would be 5 and 10 — using byte offsets directly would
        // either panic (slicing inside é) or return the wrong text.
        let src = "<?php\n$x = \"café\";";
        // "café" begins after `$x = "` (6 chars, 6 UTF-16 units, 6 bytes — all ASCII)
        let start_utf16 = 6u32; // right after the opening quote
        // "café" = 4 chars = 4 UTF-16 units, but 5 bytes
        let end_utf16 = start_utf16 + 4;
        let r = range(1, start_utf16, 1, end_utf16);
        let actions = extract_variable_actions(src, r, &uri());
        // The extracted text must be exactly "café", not a garbled slice.
        if let Some(CodeActionOrCommand::CodeAction(a)) = actions.first() {
            let edits = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let texts: Vec<&str> = edits
                .values()
                .next()
                .unwrap()
                .iter()
                .map(|e| e.new_text.as_str())
                .collect();
            assert!(
                texts.iter().any(|t| t.contains("café")),
                "extracted text must contain the correct multibyte string, got: {texts:?}"
            );
        }
    }
}
