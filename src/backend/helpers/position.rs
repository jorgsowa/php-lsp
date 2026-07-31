//! Character/offset position math and the cursor symbol-kind heuristic.

use tower_lsp::lsp_types::{Position, Range};

use crate::navigation::references::SymbolKind;

/// Classify the symbol at `position` so `find_references` can use the right walker.
///
/// Heuristics (in priority order):
/// 1. Preceded by `->` or `?->` → `Method`
/// 2. Preceded by `::` → `Method` (static)
/// 3. Word starts with `$` → variable (returns `None`; variables are handled separately)
/// 4. First character is uppercase AND not preceded by `->` or `::` → `Class`
/// 5. Otherwise → `Function`
///
/// Falls back to `None` when the context cannot be determined.
pub(crate) fn symbol_kind_at(source: &str, position: Position, word: &str) -> Option<SymbolKind> {
    if word.starts_with('$') {
        return None; // variables handled elsewhere
    }
    let line = source.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();

    // Convert UTF-16 column to char index.
    let col = position.character as usize;
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16_col >= col {
            break;
        }
        utf16_col += ch.len_utf16();
        char_idx += 1;
    }

    // Walk left past identifier characters to find the first character before the word.
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx > 0 && is_word_char(chars[char_idx - 1]) {
        char_idx -= 1;
    }

    // Look past the end of the word to distinguish `->method()` from `->prop`.
    let word_end = {
        let mut i = char_idx;
        while i < chars.len() && is_word_char(chars[i]) {
            i += 1;
        }
        // Skip spaces before the next token.
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        i
    };
    let next_is_call = word_end < chars.len() && chars[word_end] == '(';

    // Check for `->` or `?->`
    if char_idx >= 2 && chars[char_idx - 1] == '>' && chars[char_idx - 2] == '-' {
        return if next_is_call {
            Some(SymbolKind::Method)
        } else {
            Some(SymbolKind::Property)
        };
    }
    if char_idx >= 3
        && chars[char_idx - 1] == '>'
        && chars[char_idx - 2] == '-'
        && chars[char_idx - 3] == '?'
    {
        return if next_is_call {
            Some(SymbolKind::Method)
        } else {
            Some(SymbolKind::Property)
        };
    }

    // Check for `::`
    if char_idx >= 2 && chars[char_idx - 1] == ':' && chars[char_idx - 2] == ':' {
        // A `::` followed immediately by `(` is a static method call.  Without
        // `(` the identifier is a class constant access — constants are accessed
        // without parentheses in PHP (`Class::CONST`).
        return if next_is_call {
            Some(SymbolKind::Method)
        } else {
            Some(SymbolKind::Constant)
        };
    }

    // If the word starts with an uppercase letter it is likely a class/interface/enum name.
    if word
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        return Some(SymbolKind::Class);
    }

    // Otherwise treat as a free function.
    Some(SymbolKind::Function)
}

/// Convert an LSP `Position` to a byte offset within `source`, returning `None`
/// when `position.line` is past the end of `source`.
///
/// This is the strict counterpart to [`crate::text::position_to_byte_offset`],
/// which instead clamps an out-of-range line to `source.len()`. Use this variant
/// for cursor lookups, where a position outside the document means "nothing
/// here"; columns past the end of a line still clamp to the line's end.
pub(crate) fn position_to_byte_offset_strict(source: &str, position: Position) -> Option<u32> {
    let mut line_start = 0usize;
    for _ in 0..position.line {
        let i = source[line_start..].find('\n')?;
        line_start += i + 1;
    }
    let line_end = source[line_start..]
        .find('\n')
        .map_or(source.len(), |i| line_start + i);
    // Strip a trailing \r so CRLF columns count like LF columns.
    let line_content = source[line_start..line_end].trim_end_matches('\r');
    let byte =
        line_start + crate::text::utf16_offset_to_byte(line_content, position.character as usize);
    Some(byte as u32)
}

/// Returns `true` when `inner` is fully contained inside `outer` (the LSP
/// half-open `[start, end)` convention is irrelevant here — a range with
/// the exact same bounds counts as contained).
pub(crate) fn range_within(inner: Range, outer: Range) -> bool {
    let start_ok =
        (inner.start.line, inner.start.character) >= (outer.start.line, outer.start.character);
    let end_ok = (inner.end.line, inner.end.character) <= (outer.end.line, outer.end.character);
    start_ok && end_ok
}
