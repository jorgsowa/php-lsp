use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp::lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};

use crate::ast::ParsedDoc;
use crate::navigation::references::find_references_with_use;
use crate::util::utf16_code_units;
use crate::walk::{collect_var_refs_in_scope, property_refs_in_stmts};

/// Compute a WorkspaceEdit that renames every occurrence of `word` to `new_name`
/// across all open documents (including the declaration site).
pub fn rename(
    word: &str,
    new_name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    target_fqn: Option<&str>,
) -> WorkspaceEdit {
    use crate::navigation::references::find_references_with_target;

    let locations = match target_fqn {
        Some(fqn) => find_references_with_target(word, all_docs, true, None, fqn),
        None => find_references_with_use(word, all_docs, true),
    };

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for loc in locations {
        changes.entry(loc.uri).or_default().push(TextEdit {
            range: loc.range,
            new_text: new_name.to_string(),
        });
    }

    WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }
}

/// Returns the range of the word at `position` if it's a renameable symbol.
/// Used for `textDocument/prepareRename`.
pub fn prepare_rename(source: &str, position: Position) -> Option<Range> {
    use crate::util::word_at_position;
    let word = word_at_position(source, position)?;
    if word.contains('\\') {
        return None;
    }
    // PHP keywords cannot be renamed; return None so editors disable the action.
    if is_php_keyword(&word) {
        return None;
    }
    let line = source.lines().nth(position.line as usize)?;
    let col = position.character as usize;
    let chars: Vec<char> = line.chars().collect();
    // `is_word` intentionally excludes `$` so the range covers only the bare
    // identifier name (not the sigil). `word_at` may return `$var` with the `$`,
    // so we strip it before computing the range length to avoid an off-by-one.
    let is_word = |c: char| c.is_alphanumeric() || c == '_';

    // Find the character index at or before the cursor position (in UTF-16 code units)
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for (i, ch) in chars.iter().enumerate() {
        // Check if cursor is within this character's UTF-16 span
        let char_width = ch.len_utf16();
        if utf16_col + char_width > col {
            char_idx = i;
            break;
        }
        utf16_col += char_width;
        char_idx = i + 1;
    }

    // Find the start of the word by walking backwards
    let mut left = char_idx;
    while left > 0 && is_word(chars[left - 1]) {
        left -= 1;
    }

    let bare_word = word.trim_start_matches('$');
    let start_utf16: u32 = chars[..left].iter().map(|c| c.len_utf16() as u32).sum();
    let end_utf16: u32 = start_utf16 + utf16_code_units(bare_word);
    Some(Range {
        start: Position {
            line: position.line,
            character: start_utf16,
        },
        end: Position {
            line: position.line,
            character: end_utf16,
        },
    })
}

fn is_php_keyword(word: &str) -> bool {
    matches!(
        word,
        "abstract"
            | "and"
            | "array"
            | "as"
            | "break"
            | "callable"
            | "case"
            | "catch"
            | "class"
            | "clone"
            | "const"
            | "continue"
            | "declare"
            | "default"
            | "die"
            | "do"
            | "echo"
            | "else"
            | "elseif"
            | "empty"
            | "enddeclare"
            | "endfor"
            | "endforeach"
            | "endif"
            | "endswitch"
            | "endwhile"
            | "enum"
            | "eval"
            | "exit"
            | "extends"
            | "final"
            | "finally"
            | "fn"
            | "for"
            | "foreach"
            | "function"
            | "global"
            | "goto"
            | "if"
            | "implements"
            | "include"
            | "include_once"
            | "instanceof"
            | "insteadof"
            | "interface"
            | "isset"
            | "list"
            | "match"
            | "namespace"
            | "new"
            | "null"
            | "or"
            | "print"
            | "private"
            | "protected"
            | "public"
            | "readonly"
            | "require"
            | "require_once"
            | "return"
            | "self"
            | "static"
            | "switch"
            | "throw"
            | "trait"
            | "true"
            | "false"
            | "try"
            | "use"
            | "var"
            | "while"
            | "xor"
            | "yield"
    )
}

/// Rename a `$variable` (or parameter) within its enclosing function/method scope.
/// Only produces edits within the single document `uri`; variables don't cross files.
pub fn rename_variable(
    var_name: &str,
    new_name: &str,
    uri: &Url,
    doc: &ParsedDoc,
    position: Position,
) -> WorkspaceEdit {
    let bare = var_name.trim_start_matches('$');
    let new_bare = new_name.trim_start_matches('$');
    let new_text = format!("${new_bare}");

    let stmts = &doc.program().stmts;
    let sv = doc.view();
    let byte_off = sv.byte_of_position(position) as usize;

    let mut spans = Vec::new();
    collect_var_refs_in_scope(stmts, bare, byte_off, &mut spans);

    let mut seen = std::collections::HashSet::new();
    let mut edits: Vec<TextEdit> = spans
        .into_iter()
        .filter_map(|(span, _)| {
            let start = sv.position_of(span.start);
            let end = sv.position_of(span.end);
            seen.insert((start.line, start.character))
                .then_some(TextEdit {
                    range: Range { start, end },
                    new_text: new_text.clone(),
                })
        })
        .collect();
    edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));

    let mut changes = HashMap::new();
    if !edits.is_empty() {
        changes.insert(uri.clone(), edits);
    }

    WorkspaceEdit {
        changes: if changes.is_empty() {
            None
        } else {
            Some(changes)
        },
        ..Default::default()
    }
}

/// Rename a property (`->prop` / `?->prop` / class declaration) across all indexed
/// documents.  Unlike variable rename, properties are not scope-bound and may appear
/// in many files.
pub fn rename_property(
    prop_name: &str,
    new_name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for (uri, doc) in all_docs {
        let sv = doc.view();
        let mut spans = Vec::new();
        property_refs_in_stmts(sv.source(), &doc.program().stmts, prop_name, &mut spans);
        if !spans.is_empty() {
            let mut seen = std::collections::HashSet::new();
            let mut edits: Vec<TextEdit> = spans
                .into_iter()
                .filter_map(|span| {
                    let start = sv.position_of(span.start);
                    let end = sv.position_of(span.end);
                    seen.insert((start.line, start.character))
                        .then_some(TextEdit {
                            range: Range { start, end },
                            new_text: new_name.to_string(),
                        })
                })
                .collect();
            edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));
            changes.insert(uri.clone(), edits);
        }
    }
    WorkspaceEdit {
        changes: if changes.is_empty() {
            None
        } else {
            Some(changes)
        },
        ..Default::default()
    }
}
