//! Character/offset position math and the cursor symbol-kind heuristic.

use tower_lsp_server::ls_types::{Position, Range};

use crate::navigation::references::SymbolKind;

/// Whether the word at `position` is a bare PHP reserved keyword — i.e. not
/// reached via `->`/`?->`/`::`, where PHP allows reserved words as property
/// or method names (`$obj->class`, `$obj->list()`).
///
/// Callers use this to skip mir's usage-symbol lookup entirely for keyword
/// tokens: `symbol_kind_at` below already rejects a bare keyword, but mir's
/// own per-file analysis resolves the offset first and its declaration span
/// for the entity a keyword modifies can swallow the token — e.g. `abstract`
/// in `abstract class Foo` sits inside `Foo`'s class-declaration span, so
/// `symbol_at(offset)` hands back `Foo` before `symbol_kind_at` ever runs.
/// Gating on the raw token here avoids that: it never resolves to *any*
/// symbol, real or wrong, and never pays for the reference search that
/// would otherwise follow.
pub(crate) fn is_bare_keyword_at(source: &str, position: Position, word: &str) -> bool {
    if !is_php_keyword(word) {
        return false;
    }
    let Some(line) = source.lines().nth(position.line as usize) else {
        return false;
    };
    let chars: Vec<char> = line.chars().collect();
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
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx > 0 && is_word_char(chars[char_idx - 1]) {
        char_idx -= 1;
    }
    let preceded_by_arrow =
        char_idx >= 2 && chars[char_idx - 1] == '>' && chars[char_idx - 2] == '-';
    let preceded_by_nullsafe_arrow = char_idx >= 3
        && chars[char_idx - 1] == '>'
        && chars[char_idx - 2] == '-'
        && chars[char_idx - 3] == '?';
    let preceded_by_double_colon =
        char_idx >= 2 && chars[char_idx - 1] == ':' && chars[char_idx - 2] == ':';
    !(preceded_by_arrow || preceded_by_nullsafe_arrow || preceded_by_double_colon)
}

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

    // A bare reserved word (not part of `->`/`::` access, handled above) is
    // never a resolvable symbol — e.g. `final`/`readonly`/`class` in
    // `final readonly class Foo`. Without this check they fall through to
    // the free-function guess below and mir searches the whole workspace
    // for anything named e.g. "final", surfacing unrelated symbols like a
    // `$final` property.
    if is_php_keyword(word) {
        return None;
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

/// Whether `word` is a PHP reserved keyword (case-insensitive), per
/// <https://www.php.net/manual/en/reserved.keywords.php> and the "other
/// reserved words" list on the same page. These can never be the name of a
/// user-defined function/class/property, so they carry no references.
fn is_php_keyword(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
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
            | "or"
            | "print"
            | "private"
            | "protected"
            | "public"
            | "readonly"
            | "require"
            | "require_once"
            | "return"
            | "static"
            | "switch"
            | "throw"
            | "trait"
            | "try"
            | "unset"
            | "use"
            | "var"
            | "while"
            | "xor"
            | "yield"
            | "int"
            | "float"
            | "bool"
            | "string"
            | "true"
            | "false"
            | "null"
            | "void"
            | "iterable"
            | "object"
            | "mixed"
            | "never"
            | "self"
            | "parent"
    )
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
