/// Code action: "Organize imports" — sorts `use` statements alphabetically
/// and removes ones whose short name doesn't appear in the file body.
use std::collections::HashMap;

use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::util::fqn_short_name;

/// Analyse `source` and return an "Organize imports" code action if there is
/// something to do (sort order is wrong or unused imports exist).
pub fn organize_imports_action(source: &str, uri: &Url) -> Option<CodeActionOrCommand> {
    let block = find_use_block(source)?;
    if block.statements.is_empty() {
        return None;
    }

    let body_start_byte = block.body_start_byte;
    let body = &source[body_start_byte..];

    // Split into three groups by kind, filtering unused imports.
    let mut class_uses: Vec<UseStatement> = Vec::new();
    let mut fn_uses: Vec<UseStatement> = Vec::new();
    let mut const_uses: Vec<UseStatement> = Vec::new();

    for u in block.statements {
        if !is_used(&u, body) {
            continue;
        }
        match u.kind {
            UseKind::Class => class_uses.push(u),
            UseKind::Function => fn_uses.push(u),
            UseKind::Const => const_uses.push(u),
        }
    }

    // If everything was removed, emit an empty replacement.
    if class_uses.is_empty() && fn_uses.is_empty() && const_uses.is_empty() {
        let edit = TextEdit {
            range: block.range,
            new_text: String::new(),
        };
        return Some(make_action(uri, edit));
    }

    // Sort and deduplicate each group alphabetically (case-insensitive).
    sort_and_dedup(&mut class_uses);
    sort_and_dedup(&mut fn_uses);
    sort_and_dedup(&mut const_uses);

    // Build the output: class uses → (blank line) → function uses → (blank line) → const uses.
    let indent = block.indent.clone();
    let mut parts: Vec<String> = Vec::new();

    if !class_uses.is_empty() {
        parts.push(render_group(&class_uses, &indent, None));
    }
    if !fn_uses.is_empty() {
        parts.push(render_group(&fn_uses, &indent, Some("function")));
    }
    if !const_uses.is_empty() {
        parts.push(render_group(&const_uses, &indent, Some("const")));
    }

    // Join groups with a blank line between them.
    let indented = parts.join("\n");

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

/// Render a group of use statements as a string block ending with a trailing newline.
fn render_group(stmts: &[UseStatement], indent: &str, keyword: Option<&str>) -> String {
    stmts
        .iter()
        .map(|u| {
            let kw = match keyword {
                Some(k) => format!("{k} "),
                None => String::new(),
            };
            let stmt = if let Some(alias) = &u.alias {
                format!("use {}{} as {};\n", kw, u.fqn, alias)
            } else {
                format!("use {}{};\n", kw, u.fqn)
            };
            if indent.is_empty() {
                stmt
            } else {
                format!("{indent}{stmt}")
            }
        })
        .collect()
}

/// Sort a group alphabetically (case-insensitive) and deduplicate by FQN.
fn sort_and_dedup(group: &mut Vec<UseStatement>) {
    group.sort_by_cached_key(|u| u.fqn.to_lowercase());
    group.dedup_by(|a, b| a.fqn.eq_ignore_ascii_case(&b.fqn));
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

/// The kind of a `use` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
enum UseKind {
    Class,
    Function,
    Const,
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
    /// Whether this is a class, function, or const import.
    kind: UseKind,
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

            // Detect `use function` / `use const`.
            let (kind, stmt_text) = if let Some(fn_rest) = rest.strip_prefix("function ") {
                (UseKind::Function, fn_rest.trim_end_matches(';').trim())
            } else if let Some(const_rest) = rest.strip_prefix("const ") {
                (UseKind::Const, const_rest.trim_end_matches(';').trim())
            } else {
                (UseKind::Class, rest.trim_end_matches(';').trim())
            };

            if let Some(us) = parse_use_statement(stmt_text, kind) {
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

fn parse_use_statement(text: &str, kind: UseKind) -> Option<UseStatement> {
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
        None => fqn_short_name(fqn_part).to_string(),
    };

    Some(UseStatement {
        fqn: fqn_part.to_string(),
        alias,
        short,
        kind,
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
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'\\');
        let after_ok = body
            .as_bytes()
            .get(abs + short.len())
            .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_');
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
    if current == line_no {
        offset
    } else {
        source.len()
    }
}

/// Convert a UTF-16 column offset to a byte offset within a line that starts at `line_start`.
fn utf16_col_to_byte(source: &str, line_start: usize, utf16_col: u32) -> usize {
    let mut byte_off = line_start;
    let mut col = 0u32;
    for ch in source[line_start..].chars() {
        if ch == '\n' || ch == '\r' || col >= utf16_col {
            break;
        }
        col += ch.len_utf16() as u32;
        byte_off += ch.len_utf8();
    }
    byte_off
}

/// Convert an LSP `Range` to a byte range in `source`.
fn byte_range_of(source: &str, range: Range) -> std::ops::Range<usize> {
    let start = utf16_col_to_byte(
        source,
        line_start_byte(source, range.start.line),
        range.start.character,
    );
    let end = utf16_col_to_byte(
        source,
        line_start_byte(source, range.end.line),
        range.end.character,
    );
    start..end.min(source.len())
}
