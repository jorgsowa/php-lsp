/// Code action: "Organize imports" — sorts `use` statements alphabetically
/// and removes ones whose short name doesn't appear in the file body.
use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

/// Analyse `source` and return an "Organize imports" code action if there is
/// something to do (sort order is wrong or unused imports exist).
pub fn organize_imports_action(source: &str, uri: &Url) -> Option<CodeActionOrCommand> {
    let block = find_use_block(source)?;
    if block.statements.is_empty() {
        return None;
    }

    let body_start_byte = block.body_start_byte;
    let body = &source[body_start_byte..];

    let mut kept: Vec<UseStatement> = block
        .statements
        .into_iter()
        .filter(|u| is_used(u, body))
        .collect();

    if kept.is_empty() {
        // Remove the entire use block (replace with empty string).
        // Guard: don't produce an action that deletes everything.
        // Only act if there really were use-statements.
        let edit = TextEdit {
            range: block.range,
            new_text: String::new(),
        };
        return Some(make_action(uri, edit));
    }

    // Sort: group by leading namespace segment, then alphabetically within group.
    kept.sort_by(|a, b| a.fqn.to_lowercase().cmp(&b.fqn.to_lowercase()));

    let sorted_text: String = kept
        .iter()
        .map(|u| {
            if let Some(alias) = &u.alias {
                format!("use {} as {};\n", u.fqn, alias)
            } else {
                format!("use {};\n", u.fqn)
            }
        })
        .collect();

    // Preserve leading indentation from the first use statement.
    let indent = block.indent.clone();
    let indented: String = if indent.is_empty() {
        sorted_text
    } else {
        sorted_text
            .lines()
            .map(|l| format!("{indent}{l}\n"))
            .collect()
    };

    // Only emit an action if something actually changed.
    let current_text = &source[byte_range_of(source, block.range)];
    if current_text == indented {
        return None;
    }

    let edit = TextEdit {
        range: block.range,
        new_text: indented,
    };
    Some(make_action(uri, edit))
}

fn make_action(uri: &Url, edit: TextEdit) -> CodeActionOrCommand {
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    CodeActionOrCommand::CodeAction(CodeAction {
        title: "Organize imports".to_string(),
        kind: Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// A single parsed `use` statement.
#[derive(Debug, Clone)]
struct UseStatement {
    /// The fully-qualified name, e.g. `App\Services\Mailer`.
    fqn: String,
    /// Optional alias from `use Foo as Bar`.
    alias: Option<String>,
    /// The short (unaliased) name used in the code body, e.g. `Mailer` or `Bar`.
    short: String,
}

struct UseBlock {
    /// The LSP range covering the entire use block (all `use` lines).
    range: Range,
    /// Parsed use statements in source order.
    statements: Vec<UseStatement>,
    /// Leading whitespace (indentation) of the first use line.
    indent: String,
    /// Byte offset where the file body starts (after the use block).
    body_start_byte: usize,
}

/// Scan `source` for a contiguous block of `use` statements.
/// Returns `None` if there are none.
fn find_use_block(source: &str) -> Option<UseBlock> {
    let mut first_line: Option<u32> = None;
    let mut last_line: Option<u32> = None;
    let mut statements: Vec<UseStatement> = Vec::new();
    let mut indent = String::new();

    for (idx, line) in source.lines().enumerate() {
        let line_no = idx as u32;
        let trimmed = line.trim();

        // Skip blank lines within (or between) use blocks.
        if trimmed.is_empty() {
            continue;
        }

        // A `use` statement: `use Foo\Bar;` or `use Foo\Bar as Baz;`
        // Exclude `use` inside classes/closures — those start with modifiers.
        if let Some(rest) = trimmed.strip_prefix("use ") {
            // Reject trait-use (`use TraitName;` inside a class body without \)
            // and closure-use (`use ($var)`).
            if rest.trim_start().starts_with('(') {
                // Closure use — skip.
                if first_line.is_some() {
                    break;
                }
                continue;
            }
            // Reject `use function` / `use const` for now (keep simple).
            if rest.starts_with("function ") || rest.starts_with("const ") {
                continue;
            }

            let stmt_text = rest.trim_end_matches(';').trim();
            if let Some(us) = parse_use_statement(stmt_text) {
                if first_line.is_none() {
                    first_line = Some(line_no);
                    indent = line
                        .chars()
                        .take_while(|c| c.is_whitespace())
                        .collect::<String>();
                }
                last_line = Some(line_no);
                statements.push(us);
            }
            continue;
        }

        // Non-use, non-blank line after we've started collecting — stop.
        if first_line.is_some() {
            break;
        }
    }

    let first = first_line?;
    let last = last_line?;

    // Compute the LSP range: from start of first use line to end of last use line.
    let range = Range {
        start: Position {
            line: first,
            character: 0,
        },
        end: Position {
            line: last + 1,
            character: 0,
        },
    };

    // Body start: byte offset of the line after the last use line.
    let body_start_byte = line_start_byte(source, last + 1);

    Some(UseBlock {
        range,
        statements,
        indent,
        body_start_byte,
    })
}

fn parse_use_statement(text: &str) -> Option<UseStatement> {
    // Handle `Foo\Bar as Baz` and plain `Foo\Bar`.
    let (fqn_part, alias) = if let Some((fqn, al)) = text.split_once(" as ") {
        (fqn.trim(), Some(al.trim().to_string()))
    } else {
        (text.trim(), None)
    };

    if fqn_part.is_empty() {
        return None;
    }

    let short = match &alias {
        Some(a) => a.clone(),
        None => fqn_part
            .rsplit('\\')
            .next()
            .unwrap_or(fqn_part)
            .to_string(),
    };

    Some(UseStatement {
        fqn: fqn_part.to_string(),
        alias,
        short,
    })
}

/// Returns `true` if `u.short` appears in the file body (after the use block).
fn is_used(u: &UseStatement, body: &str) -> bool {
    // Simple substring check: the short name must appear as a whole word.
    let short = &u.short;
    let mut start = 0;
    while let Some(pos) = body[start..].find(short.as_str()) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !body
                .as_bytes()
                .get(abs - 1)
                .map_or(false, |b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'\\');
        let after_ok = body
            .as_bytes()
            .get(abs + short.len())
            .map_or(true, |b| !b.is_ascii_alphanumeric() && *b != b'_');
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Return the byte offset of the start of line `line_no` (0-indexed).
fn line_start_byte(source: &str, line_no: u32) -> usize {
    let mut current = 0u32;
    let mut offset = 0;
    for (i, c) in source.char_indices() {
        if current == line_no {
            return i;
        }
        if c == '\n' {
            current += 1;
            offset = i + 1;
        }
    }
    if current == line_no { offset } else { source.len() }
}

/// Convert an LSP `Range` to a byte range in `source`.
fn byte_range_of(source: &str, range: Range) -> std::ops::Range<usize> {
    let start = line_start_byte(source, range.start.line) + range.start.character as usize;
    let end = line_start_byte(source, range.end.line) + range.end.character as usize;
    start..end.min(source.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
    }

    #[test]
    fn no_use_statements_returns_none() {
        let src = "<?php\n\nclass Foo {}\n";
        assert!(organize_imports_action(src, &uri()).is_none());
    }

    #[test]
    fn already_sorted_single_import_returns_none() {
        let src = "<?php\nuse App\\Mailer;\n\n$m = new Mailer();\n";
        // Already sorted and used — no change needed.
        assert!(organize_imports_action(src, &uri()).is_none());
    }

    #[test]
    fn unsorted_imports_are_sorted() {
        let src = "<?php\nuse App\\Zebra;\nuse App\\Alpha;\n\n$a = new Alpha();\n$z = new Zebra();\n";
        let action = organize_imports_action(src, &uri());
        assert!(action.is_some(), "should produce an action");
        let CodeActionOrCommand::CodeAction(ca) = action.unwrap() else {
            panic!("expected CodeAction");
        };
        let edits = ca
            .edit
            .unwrap()
            .changes
            .unwrap()
            .into_values()
            .next()
            .unwrap();
        let new_text = &edits[0].new_text;
        let alpha_pos = new_text.find("Alpha").unwrap();
        let zebra_pos = new_text.find("Zebra").unwrap();
        assert!(alpha_pos < zebra_pos, "Alpha should come before Zebra");
    }

    #[test]
    fn unused_import_is_removed() {
        let src = "<?php\nuse App\\Mailer;\nuse App\\Logger;\n\n$m = new Mailer();\n";
        // Logger is unused; Mailer is used.
        let action = organize_imports_action(src, &uri());
        assert!(action.is_some(), "should produce an action to remove Logger");
        let CodeActionOrCommand::CodeAction(ca) = action.unwrap() else {
            panic!("expected CodeAction");
        };
        let edits = ca
            .edit
            .unwrap()
            .changes
            .unwrap()
            .into_values()
            .next()
            .unwrap();
        let new_text = &edits[0].new_text;
        assert!(!new_text.contains("Logger"), "Logger should be removed");
        assert!(new_text.contains("Mailer"), "Mailer should be kept");
    }

    #[test]
    fn aliased_import_uses_alias_for_usage_check() {
        let src = "<?php\nuse App\\Mailer as Mail;\n\n$m = new Mail();\n";
        // 'Mail' (the alias) is used — should be kept.
        assert!(
            organize_imports_action(src, &uri()).is_none(),
            "used aliased import should not be removed"
        );
    }

    #[test]
    fn aliased_import_kept_with_alias_syntax() {
        let src = "<?php\nuse App\\Zebra as Z;\nuse App\\Alpha;\n\n$a = new Alpha();\n$z = new Z();\n";
        let action = organize_imports_action(src, &uri());
        assert!(action.is_some());
        let CodeActionOrCommand::CodeAction(ca) = action.unwrap() else {
            panic!("expected CodeAction");
        };
        let edits = ca
            .edit
            .unwrap()
            .changes
            .unwrap()
            .into_values()
            .next()
            .unwrap();
        let new_text = &edits[0].new_text;
        assert!(new_text.contains("as Z"), "aliased import should keep alias syntax");
    }

    #[test]
    fn action_kind_is_source_organize_imports() {
        let src = "<?php\nuse App\\Zebra;\nuse App\\Alpha;\n\n$a = new Alpha();\n$z = new Zebra();\n";
        let CodeActionOrCommand::CodeAction(ca) = organize_imports_action(src, &uri()).unwrap() else {
            panic!("expected CodeAction");
        };
        assert_eq!(ca.kind, Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS));
    }
}
