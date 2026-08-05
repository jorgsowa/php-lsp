//! PHP reserved-word knowledge: the single source of truth for "is this
//! token a keyword, not a name" — shared by every feature that resolves a
//! cursor position to a symbol (references, rename, definition, declaration,
//! implementation, call hierarchy, hover). A bare keyword can never be a
//! user-defined function/class/property/constant name, so none of those
//! features should ever treat one as a searchable symbol.

use php_ast::BuiltinType;
use php_lexer::TokenKind;
use php_lexer::token::resolve_keyword;
use tower_lsp_server::ls_types::Position;

/// Whether `word` is a PHP reserved keyword, predefined magic constant, or
/// reserved type-hint word (case-insensitive — all three are), sourced from
/// `php-lexer`/`php-ast` (the same crates that back this workspace's parser)
/// rather than a hand-copied word list, so additions PHP itself makes land
/// here automatically the next time that dependency is bumped.
pub(crate) fn is_php_keyword(word: &str) -> bool {
    // `insteadof` (trait-adaptation conflict resolution) and legacy `var`
    // (PHP4-style property visibility) are recognized by php-rs-parser via a
    // raw text comparison against a plain `Identifier` token at their one
    // grammar site (`php-parser/src/stmt/trait_use.rs`, `stmt/class.rs`) —
    // neither is exposed as keyword data by any crate, so they stay
    // hand-listed here.
    if word.eq_ignore_ascii_case("insteadof") || word.eq_ignore_ascii_case("var") {
        return true;
    }

    // `php-lexer::resolve_keyword` also classifies `from` as a keyword
    // token, but PHP only reserves it directly after `yield` — everywhere
    // else (e.g. `Suit::from(...)` on any backed enum) it's an ordinary
    // identifier, so it must not be treated as a bare keyword here.
    if let Some(kind) = resolve_keyword(word) {
        return kind != TokenKind::From;
    }

    is_reserved_builtin_type_word(word)
}

/// Type-hint words the lexer leaves as plain `Identifier` tokens, only
/// classified by the parser when they appear in type position
/// (`php_ast::BuiltinType`).
fn is_reserved_builtin_type_word(word: &str) -> bool {
    // `Integer`/`Double`/`Boolean` are deliberately excluded: those are
    // cast-expression aliases only (`(integer)`, `(double)`, `(boolean)`),
    // never valid type hints, and — like bare `from` above — ordinary
    // identifiers everywhere else. See `builtin_type_exhaustiveness_guard`
    // in the test module: it fails to compile if `BuiltinType` ever grows a
    // variant this list hasn't accounted for.
    const RESERVED: [BuiltinType; 17] = [
        BuiltinType::Int,
        BuiltinType::Float,
        BuiltinType::String,
        BuiltinType::Bool,
        BuiltinType::Void,
        BuiltinType::Never,
        BuiltinType::Mixed,
        BuiltinType::Object,
        BuiltinType::Iterable,
        BuiltinType::Callable,
        BuiltinType::Array,
        BuiltinType::Self_,
        BuiltinType::Parent_,
        BuiltinType::Static,
        BuiltinType::Null,
        BuiltinType::True,
        BuiltinType::False,
    ];
    RESERVED
        .iter()
        .any(|t| t.as_str().eq_ignore_ascii_case(word))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halt_compiler_is_recognized() {
        // Present on php.net's reserved.keywords.php but missing from the
        // old hand-copied list entirely — cursor on it fell through to the
        // same bare-name fallback lookups the other keywords are gated
        // against. `resolve_keyword` covers it for free.
        assert!(is_php_keyword("__halt_compiler"));
        assert!(is_php_keyword("__HALT_COMPILER"));
    }

    #[test]
    fn from_is_not_a_bare_keyword() {
        // The one real trap in reusing `php_lexer::resolve_keyword` wholesale:
        // it classifies `from` as a keyword token, but PHP only reserves it
        // directly after `yield`. Everywhere else — most commonly
        // `Suit::from('H')` on any PHP 8.1+ backed enum — it's an ordinary,
        // very common method/function name. Treating it as always-reserved
        // would silently break goto-definition/references/hover for any
        // symbol literally named `from`.
        assert!(!is_php_keyword("from"));
        assert!(!is_php_keyword("FROM"));
        assert!(!is_php_keyword("From"));
    }

    #[test]
    fn cast_only_type_aliases_are_not_bare_keywords() {
        // `integer`/`double`/`boolean` are valid inside cast expressions
        // (`(integer) $x`) but are never valid type hints and are not
        // reserved elsewhere — same trap as `from`. `BuiltinType` models all
        // three as variants (for cast parsing), but they must not leak into
        // this bare-keyword gate.
        assert!(!is_php_keyword("integer"));
        assert!(!is_php_keyword("double"));
        assert!(!is_php_keyword("boolean"));
    }

    #[test]
    fn insteadof_and_legacy_var_are_recognized() {
        // Neither is exposed as keyword data by php-lexer or php-ast — both
        // are recognized by php-rs-parser via raw text comparison at their
        // one grammar site, so they stay hand-listed in `is_php_keyword`.
        assert!(is_php_keyword("insteadof"));
        assert!(is_php_keyword("InsteadOf"));
        assert!(is_php_keyword("var"));
        assert!(is_php_keyword("VAR"));
    }

    #[test]
    fn builtin_type_hint_words_are_recognized() {
        for word in [
            "int", "float", "bool", "string", "void", "never", "mixed", "object", "iterable",
            "callable", "array", "self", "parent", "static", "null", "true", "false",
        ] {
            assert!(
                is_php_keyword(word),
                "{word} should be a reserved type-hint word"
            );
            assert!(
                is_php_keyword(&word.to_ascii_uppercase()),
                "{word} should be reserved case-insensitively"
            );
        }
    }

    #[test]
    fn magic_constants_are_recognized() {
        for word in [
            "__class__",
            "__dir__",
            "__file__",
            "__function__",
            "__line__",
            "__method__",
            "__namespace__",
            "__trait__",
        ] {
            assert!(
                is_php_keyword(word),
                "{word} should be a recognized magic constant"
            );
        }
    }

    #[test]
    fn ordinary_statement_and_modifier_keywords_still_recognized() {
        // Regression pin against the switch from a hand-copied list to
        // `resolve_keyword`-backed lookup: every word the old list already
        // covered must still come back true.
        for word in [
            "abstract",
            "and",
            "as",
            "break",
            "case",
            "catch",
            "class",
            "clone",
            "const",
            "continue",
            "declare",
            "default",
            "die",
            "do",
            "echo",
            "else",
            "elseif",
            "empty",
            "enddeclare",
            "endfor",
            "endforeach",
            "endif",
            "endswitch",
            "endwhile",
            "enum",
            "eval",
            "exit",
            "extends",
            "final",
            "finally",
            "fn",
            "for",
            "foreach",
            "function",
            "global",
            "goto",
            "if",
            "implements",
            "include",
            "include_once",
            "instanceof",
            "interface",
            "isset",
            "list",
            "match",
            "namespace",
            "new",
            "or",
            "print",
            "private",
            "protected",
            "public",
            "readonly",
            "require",
            "require_once",
            "return",
            "switch",
            "throw",
            "trait",
            "try",
            "unset",
            "use",
            "while",
            "xor",
            "yield",
        ] {
            assert!(
                is_php_keyword(word),
                "{word} regressed from the old keyword list"
            );
        }
    }

    #[test]
    fn ordinary_identifiers_are_not_keywords() {
        for word in ["myFunction", "Suit", "handle", "PHP_EOL_ish", "runner", ""] {
            assert!(
                !is_php_keyword(word),
                "{word} must not be treated as a keyword"
            );
        }
    }

    // Exhaustive match with no wildcard arm: if `php_ast::BuiltinType` ever
    // grows a new variant, this fails to compile until someone decides
    // whether it belongs in `is_reserved_builtin_type_word`'s `RESERVED`
    // list — the same silent-gap class of bug that let `__halt_compiler` go
    // missing from the old hand-copied list.
    #[allow(dead_code)]
    fn builtin_type_exhaustiveness_guard(t: BuiltinType) -> bool {
        match t {
            BuiltinType::Int
            | BuiltinType::Float
            | BuiltinType::String
            | BuiltinType::Bool
            | BuiltinType::Void
            | BuiltinType::Never
            | BuiltinType::Mixed
            | BuiltinType::Object
            | BuiltinType::Iterable
            | BuiltinType::Callable
            | BuiltinType::Array
            | BuiltinType::Self_
            | BuiltinType::Parent_
            | BuiltinType::Static
            | BuiltinType::Null
            | BuiltinType::True
            | BuiltinType::False => true,
            BuiltinType::Integer | BuiltinType::Double | BuiltinType::Boolean => false,
        }
    }
}
