//! Word/identifier extraction under the cursor, parameter-list splitting, and
//! PHP-name string helpers (short-name / variable-sigil stripping).

use tower_lsp_server::ls_types::{Position, Range};

use super::offset::utf16_offset_to_byte;

/// Split a rendered parameter list on its top-level commas, each part trimmed.
///
/// Commas nested in `()`, `[]`, `{}` or string literals never split. Inside a
/// parameter's type (before its `$name`) `<...>` nests too, as in
/// `array<int, string>`; after the name, `<` and `>` are operators in the
/// default value (`=>`, `<<`, `1 < 2`) and are ignored.
pub(crate) fn split_params(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut angle = 0i32;
    let mut in_type = true;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' if in_type => angle += 1,
            '>' if in_type && angle > 0 => angle -= 1,
            '$' if depth == 0 && angle == 0 => in_type = false,
            ',' if depth == 0 && angle == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
                in_type = true;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    parts
}

/// Extract the word (identifier) under the cursor, handling UTF-16 offsets.
fn char_range_for_word(line: &str, char_offset: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let char_pos = crate::text::utf16_col_to_char_index(&chars, char_offset);
    let total_utf16: usize = chars.iter().map(|c| c.len_utf16()).sum();
    if char_offset > total_utf16 {
        return None;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '\\';
    let mut left = char_pos;
    while left > 0 && is_word(chars[left - 1]) {
        left -= 1;
    }
    let mut right = char_pos;
    while right < chars.len() && is_word(chars[right]) {
        right += 1;
    }
    if left == right {
        None
    } else {
        Some((left, right))
    }
}

pub(crate) fn word_at_position(source: &str, position: Position) -> Option<String> {
    // Use split('\n') rather than lines() so that a trailing newline produces a
    // final empty entry — lines() silently drops it, causing word_at_position to return
    // None for any cursor on the last line of a normally-saved PHP file.
    let raw = source.split('\n').nth(position.line as usize)?;
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    let char_offset = position.character as usize;
    let chars: Vec<char> = line.chars().collect();
    let (left, right) = char_range_for_word(line, char_offset)?;
    let word: String = chars[left..right].iter().collect();
    if word.is_empty() { None } else { Some(word) }
}

/// Return the LSP `Range` of the word (identifier) under the cursor.
/// Uses the same word-boundary rules as `word_at_position`.
pub(crate) fn word_range_at(source: &str, position: Position) -> Option<Range> {
    let raw = source.split('\n').nth(position.line as usize)?;
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    let char_offset = position.character as usize;
    let chars: Vec<char> = line.chars().collect();
    let (left, right) = char_range_for_word(line, char_offset)?;
    let start_col = chars[..left]
        .iter()
        .map(|c| c.len_utf16() as u32)
        .sum::<u32>();
    let end_col = chars[..right]
        .iter()
        .map(|c| c.len_utf16() as u32)
        .sum::<u32>();
    Some(Range {
        start: Position {
            line: position.line,
            character: start_col,
        },
        end: Position {
            line: position.line,
            character: end_col,
        },
    })
}

/// Extract the source text covered by an LSP `Range`.
///
/// `Range` positions use UTF-16 code-unit offsets; this function converts them
/// correctly before slicing the UTF-8 source string.
pub(crate) fn selected_text_range(source: &str, range: Range) -> String {
    // split('\n') (not lines()) keeps each line's trailing '\r' so CRLF sources
    // round-trip intact; lines() strips '\r' and the rejoin below would emit '\n'.
    let lines: Vec<&str> = source.split('\n').collect();
    if range.start.line == range.end.line {
        let line = match lines.get(range.start.line as usize) {
            Some(l) => l,
            None => return String::new(),
        };
        let start = utf16_offset_to_byte(line, range.start.character as usize);
        let end = utf16_offset_to_byte(line, range.end.character as usize).max(start);
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

/// Strip the leading `$` sigil from a variable name, if present.
/// Variables are stored both ways: `$var` in source, `var` in symbol tables.
pub(crate) fn strip_variable_sigil(word: &str) -> &str {
    word.strip_prefix('$').unwrap_or(word)
}

/// Return the unqualified short name from a PHP fully-qualified name.
/// `"\App\Service\Foo"` → `"Foo"`, `"Foo"` → `"Foo"`, `""` → `""`.
pub(crate) fn fqn_short_name(fqn: &str) -> &str {
    fqn.rsplit('\\').next().unwrap_or(fqn)
}

/// `name` qualified with namespace `ns`: `("App", "Foo")` -> `App\Foo`, `("", "Foo")` -> `Foo`.
pub(crate) fn qualify_name(ns: &str, name: &str) -> String {
    if ns.is_empty() {
        name.to_string()
    } else {
        format!("{ns}\\{name}")
    }
}

/// Compare an already-resolved class identity with a declaration in one
/// document. A short name is an intentional fallback; an FQCN must match both
/// the namespace and the declaration name.
pub(crate) fn class_matches_declaration(
    target: &str,
    declared: &str,
    namespace: Option<&str>,
) -> bool {
    let target = target.trim_start_matches('\\');
    if !target.contains('\\') {
        return target.eq_ignore_ascii_case(declared);
    }
    let declared_fqcn = namespace
        .filter(|ns| !ns.is_empty())
        .map(|ns| format!("{ns}\\{declared}"))
        .unwrap_or_else(|| declared.to_string());
    target.eq_ignore_ascii_case(&declared_fqcn)
}

/// Whether `haystack` contains `needle` as an ASCII-case-insensitive
/// substring — PHP class/method names are case-insensitive (`new COLOR()`
/// resolves to `class Color`), matching the semantics mir's own candidate
/// gate and `fqn_reachable_files` already use elsewhere in this codebase.
pub(crate) fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    n.len() <= h.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_at_last_line_with_trailing_newline() {
        // Editors save files with a trailing newline; lines() drops the final
        // empty entry, making word_at return None for cursors on the last line.
        let src = "<?php\necho strlen($x);\n";
        let pos = Position {
            line: 1,
            character: 6,
        }; // "strlen" on line 1
        let w = word_at_position(src, pos);
        assert_eq!(
            w.as_deref(),
            Some("strlen"),
            "word_at_position must work on lines before the trailing newline"
        );
        // Position on the final empty line produced by the trailing newline.
        let last_line = Position {
            line: 2,
            character: 0,
        };
        // Should return None (empty line), but must not panic.
        let _ = word_at_position(src, last_line);
    }

    #[test]
    fn word_at_crlf_line_endings() {
        let src = "<?php\r\nfunction foo() {}\r\n";
        let pos = Position {
            line: 1,
            character: 9,
        }; // "foo"
        let w = word_at_position(src, pos);
        assert_eq!(
            w.as_deref(),
            Some("foo"),
            "word_at_position must handle CRLF line endings"
        );
    }

    #[test]
    fn selected_text_range_preserves_crlf() {
        let src = "<?php\r\n$a = 1;\r\n$b = 2;\r\n";
        let range = Range {
            start: Position {
                line: 1,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 7,
            },
        };
        assert_eq!(
            selected_text_range(src, range),
            "$a = 1;\r\n$b = 2;",
            "multi-line extraction must preserve CRLF line endings"
        );
    }

    #[test]
    fn selected_text_range_reversed_does_not_panic() {
        let src = "<?php $x = 1;";
        let range = Range {
            start: Position {
                line: 0,
                character: 10,
            },
            end: Position {
                line: 0,
                character: 3,
            },
        };
        assert_eq!(selected_text_range(src, range), "");
    }

    #[test]
    fn contains_ascii_case_insensitive_matches_regardless_of_case() {
        assert!(contains_ascii_case_insensitive("new COLOR()", "Color"));
        assert!(contains_ascii_case_insensitive("new Color()", "COLOR"));
        assert!(contains_ascii_case_insensitive("class Color {}", "color"));
    }

    #[test]
    fn contains_ascii_case_insensitive_rejects_non_match() {
        assert!(!contains_ascii_case_insensitive("class Node {}", "Color"));
    }

    #[test]
    fn contains_ascii_case_insensitive_empty_needle_matches_anything() {
        assert!(contains_ascii_case_insensitive("anything", ""));
        assert!(contains_ascii_case_insensitive("", ""));
    }

    #[test]
    fn split_params_keeps_generic_type_arguments_together() {
        assert_eq!(
            split_params(
                "string|array<int|string, string> $search, string|array<int|string, scalar|Stringable|null> $replace, string|array<int|string, string> $subject, mixed &$count = null"
            ),
            [
                "string|array<int|string, string> $search",
                "string|array<int|string, scalar|Stringable|null> $replace",
                "string|array<int|string, string> $subject",
                "mixed &$count = null",
            ]
        );
    }

    #[test]
    fn split_params_handles_nested_and_closing_generic_pairs() {
        assert_eq!(
            split_params("array<int, array<string, int>> $a, int<0, max> $b, Foo<A, B>|null $c"),
            [
                "array<int, array<string, int>> $a",
                "int<0, max> $b",
                "Foo<A, B>|null $c"
            ]
        );
    }

    #[test]
    fn split_params_ignores_comparison_and_shift_operators_in_defaults() {
        assert_eq!(
            split_params("int $a = 1 << 2, bool $b = 1 < 2, int $c = 8 >> 1, array<int, int> $d"),
            [
                "int $a = 1 << 2",
                "bool $b = 1 < 2",
                "int $c = 8 >> 1",
                "array<int, int> $d"
            ]
        );
    }

    #[test]
    fn split_params_ignores_arrows_in_array_defaults() {
        assert_eq!(
            split_params("array $a = ['k' => 1, 'j' => 2], array<string, int> $b = ['x' => 1]"),
            [
                "array $a = ['k' => 1, 'j' => 2]",
                "array<string, int> $b = ['x' => 1]"
            ]
        );
    }

    #[test]
    fn split_params_ignores_commas_inside_string_defaults() {
        assert_eq!(
            split_params(
                r#"string $sep = ', ', string $q = "a,\"b", string $e = 'it\'s, ok', $last"#
            ),
            [
                "string $sep = ', '",
                r#"string $q = "a,\"b""#,
                r"string $e = 'it\'s, ok'",
                "$last"
            ]
        );
    }

    #[test]
    fn split_params_keeps_shapes_and_callable_types_together() {
        assert_eq!(
            split_params(
                "array{a: int, b: list<string>} $s, callable(int, string): array<int, bool> $cb, Closure(int $x, int $y): void $c"
            ),
            [
                "array{a: int, b: list<string>} $s",
                "callable(int, string): array<int, bool> $cb",
                "Closure(int $x, int $y): void $c"
            ]
        );
    }

    #[test]
    fn split_params_keeps_string_literal_types_together() {
        assert_eq!(
            split_params("'a,b'|'c' $mode, int $n"),
            ["'a,b'|'c' $mode", "int $n"]
        );
    }

    #[test]
    fn split_params_plain_lists() {
        assert_eq!(
            split_params("$array, $callback = null, $mode = 0"),
            ["$array", "$callback = null", "$mode = 0"]
        );
        assert!(split_params("").is_empty());
        assert_eq!(split_params("...$arrays"), ["...$arrays"]);
    }
}
