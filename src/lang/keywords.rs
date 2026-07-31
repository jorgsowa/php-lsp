//! PHP reserved-word knowledge: the single source of truth for "is this
//! token a keyword, not a name" — shared by every feature that resolves a
//! cursor position to a symbol (references, rename, definition, declaration,
//! implementation, call hierarchy, hover). A bare keyword can never be a
//! user-defined function/class/property/constant name, so none of those
//! features should ever treat one as a searchable symbol.

use tower_lsp_server::ls_types::Position;

/// Whether `word` is a PHP reserved keyword or magic constant (case-
/// insensitive — PHP keywords and magic constants are both case-insensitive),
/// per <https://www.php.net/manual/en/reserved.keywords.php>,
/// <https://www.php.net/manual/en/reserved.other-words.php>, and the
/// predefined-constants magic-constant list.
pub(crate) fn is_php_keyword(word: &str) -> bool {
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
            | "__class__"
            | "__dir__"
            | "__file__"
            | "__function__"
            | "__line__"
            | "__method__"
            | "__namespace__"
            | "__trait__"
    )
}

/// Whether the word at `position` is a bare PHP reserved keyword — i.e. not
/// reached via `->`/`?->`/`::`, where PHP allows reserved words as property
/// or method names (`$obj->class`, `$obj->list()`).
///
/// Callers use this to skip symbol resolution entirely for keyword tokens:
/// a per-file/per-index analysis pass can resolve the offset to a real
/// symbol *before* any name-based heuristic runs — e.g. mir's declaration
/// span for a class starts at its first modifier token, so `abstract` in
/// `abstract class Foo` sits inside `Foo`'s class-declaration span and
/// resolves to `Foo` itself. Gating on the raw token here avoids that: it
/// never resolves to *any* symbol, real or wrong, and never pays for the
/// search that would otherwise follow.
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
