use tower_lsp::lsp_types::{Position, Range, TextEdit};

use crate::text::byte_to_utf16;

/// If `line` is a `use` statement whose FQN (after an optional leading `\`)
/// is exactly `target` on a word boundary (`;`, space, `{`, `,`, or
/// end-of-line), return the byte range of the matched FQN text.
fn find_use_match_in_line(line: &str, target: &str) -> Option<(usize, usize)> {
    if !line.trim_start().starts_with("use ") {
        return None;
    }
    let use_pos = line.find("use ")?;
    let after_use = use_pos + 4;

    let fqn_start = if line.as_bytes().get(after_use) == Some(&b'\\') {
        after_use + 1
    } else {
        after_use
    };
    let fqn_str = &line[fqn_start..];

    if !fqn_str.starts_with(target) {
        return None;
    }
    let after_fqn = &fqn_str[target.len()..];
    let is_boundary = after_fqn.is_empty()
        || matches!(after_fqn.as_bytes()[0], b';' | b' ' | b'\t' | b'{' | b',');
    if !is_boundary {
        return None;
    }

    Some((fqn_start, fqn_start + target.len()))
}

/// Return `TextEdit`s that delete the entire `use FQN;` line from `source`.
pub fn delete_use_in_source(source: &str, fqn: &str) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let clean = fqn.trim_start_matches('\\');

    for (line_idx, line) in source.lines().enumerate() {
        if find_use_match_in_line(line, clean).is_none() {
            continue;
        }

        // Delete the whole line including its newline.
        let line_u32 = line_idx as u32;
        let next_line = line_u32 + 1;
        edits.push(TextEdit {
            range: Range {
                start: Position {
                    line: line_u32,
                    character: 0,
                },
                end: Position {
                    line: next_line,
                    character: 0,
                },
            },
            new_text: String::new(),
        });
    }

    edits
}

/// Scan `source` for `use` statements that reference `old_fqn` and return
/// `TextEdit`s that replace `old_fqn` with `new_fqn` in each such line.
///
/// Handles:
/// - `use OldFqn;`
/// - `use \OldFqn;`
/// - `use OldFqn as Alias;`
pub fn use_edits_in_source(source: &str, old_fqn: &str, new_fqn: &str) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let old = old_fqn.trim_start_matches('\\');
    let new_clean = new_fqn.trim_start_matches('\\');

    for (line_idx, line) in source.lines().enumerate() {
        let Some((fqn_start, fqn_end)) = find_use_match_in_line(line, old) else {
            continue;
        };

        let line_u32 = line_idx as u32;
        edits.push(TextEdit {
            range: Range {
                start: Position {
                    line: line_u32,
                    character: byte_to_utf16(line, fqn_start),
                },
                end: Position {
                    line: line_u32,
                    character: byte_to_utf16(line, fqn_end),
                },
            },
            new_text: new_clean.to_string(),
        });
    }

    edits
}
