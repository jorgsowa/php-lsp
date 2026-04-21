use tower_lsp::lsp_types::Position;

/// Returns `true` if `query` matches `candidate` using camelCase/underscore
/// abbreviation rules.
///
/// Rules (applied in order, first match wins):
/// 1. `candidate` starts with `query` (case-insensitive prefix match).
/// 2. Every character of `query` matches either the start of a camelCase word
///    (uppercase letter preceded by lowercase) or the character after `_` in
///    the candidate.
///
/// Examples:
/// - `"GRF"` matches `"getRecentFiles"`
/// - `"str_r"` matches `"str_replace"`
/// - `"srp"` matches `"str_replace"`
pub(crate) fn fuzzy_camel_match(query: &str, candidate: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let ql: String = query.to_lowercase();
    let cl: String = candidate.to_lowercase();
    // Fast path: plain prefix
    if cl.starts_with(&ql) {
        return true;
    }
    // Camel / underscore abbreviation
    let qchars: Vec<char> = ql.chars().collect();
    let cchars: Vec<char> = candidate.chars().collect();
    let mut qi = 0usize;
    let mut ci = 0usize;
    while qi < qchars.len() && ci < cchars.len() {
        let qc = qchars[qi];
        // A "word boundary" in the candidate is: position 0, after '_', or
        // an uppercase letter after a lowercase letter (camelCase transition).
        let is_boundary = ci == 0
            || cchars[ci - 1] == '_'
            || (cchars[ci].is_uppercase() && ci > 0 && cchars[ci - 1].is_lowercase());
        if is_boundary && cchars[ci].to_lowercase().next() == Some(qc) {
            qi += 1;
        }
        ci += 1;
    }
    qi == qchars.len()
}

/// Compute a sort key for a completion item so that items matching the query
/// by plain prefix sort before camel/underscore abbreviation matches.
/// Lower string = higher priority.
pub(crate) fn camel_sort_key(query: &str, label: &str) -> String {
    let lq = query.to_lowercase();
    let ll = label.to_lowercase();
    if ll.starts_with(&lq) {
        format!("0{}", ll)
    } else {
        format!("1{}", ll)
    }
}

/// Return `true` if `name` is a known PHP built-in function.
/// Used by hover to generate php.net links.
pub(crate) fn is_php_builtin(name: &str) -> bool {
    // Sorted for binary search.
    const BUILTINS: &[&str] = &[
        "abs",
        "acos",
        "addslashes",
        "array_chunk",
        "array_combine",
        "array_diff",
        "array_fill",
        "array_fill_keys",
        "array_filter",
        "array_flip",
        "array_intersect",
        "array_key_exists",
        "array_keys",
        "array_map",
        "array_merge",
        "array_pad",
        "array_pop",
        "array_push",
        "array_reduce",
        "array_replace",
        "array_reverse",
        "array_search",
        "array_shift",
        "array_slice",
        "array_splice",
        "array_unique",
        "array_unshift",
        "array_values",
        "array_walk",
        "array_walk_recursive",
        "arsort",
        "asin",
        "asort",
        "atan",
        "atan2",
        "base64_decode",
        "base64_encode",
        "basename",
        "boolval",
        "call_user_func",
        "call_user_func_array",
        "ceil",
        "checkdate",
        "class_exists",
        "closedir",
        "compact",
        "constant",
        "copy",
        "cos",
        "date",
        "date_add",
        "date_create",
        "date_diff",
        "date_format",
        "date_sub",
        "define",
        "defined",
        "die",
        "dirname",
        "empty",
        "exit",
        "exp",
        "explode",
        "extract",
        "fclose",
        "feof",
        "fgets",
        "file_exists",
        "file_get_contents",
        "file_put_contents",
        "floatval",
        "floor",
        "fmod",
        "fopen",
        "fputs",
        "fread",
        "fseek",
        "ftell",
        "function_exists",
        "get_class",
        "get_parent_class",
        "gettype",
        "glob",
        "hash",
        "header",
        "headers_sent",
        "htmlentities",
        "htmlspecialchars",
        "http_build_query",
        "implode",
        "in_array",
        "intdiv",
        "interface_exists",
        "intval",
        "is_a",
        "is_array",
        "is_bool",
        "is_callable",
        "is_dir",
        "is_double",
        "is_file",
        "is_finite",
        "is_float",
        "is_infinite",
        "is_int",
        "is_integer",
        "is_long",
        "is_nan",
        "is_null",
        "is_numeric",
        "is_object",
        "is_readable",
        "is_string",
        "is_subclass_of",
        "is_writable",
        "isset",
        "join",
        "json_decode",
        "json_encode",
        "krsort",
        "ksort",
        "lcfirst",
        "list",
        "log",
        "ltrim",
        "max",
        "md5",
        "method_exists",
        "microtime",
        "min",
        "mkdir",
        "mktime",
        "mt_rand",
        "nl2br",
        "number_format",
        "ob_end_clean",
        "ob_get_clean",
        "ob_start",
        "opendir",
        "parse_str",
        "parse_url",
        "pathinfo",
        "pi",
        "pow",
        "preg_match",
        "preg_match_all",
        "preg_quote",
        "preg_replace",
        "preg_split",
        "print_r",
        "printf",
        "property_exists",
        "rand",
        "random_int",
        "rawurldecode",
        "rawurlencode",
        "readdir",
        "realpath",
        "rename",
        "rewind",
        "rmdir",
        "round",
        "rsort",
        "rtrim",
        "scandir",
        "serialize",
        "session_destroy",
        "session_start",
        "setcookie",
        "settype",
        "sha1",
        "sin",
        "sleep",
        "sort",
        "sprintf",
        "sqrt",
        "str_contains",
        "str_ends_with",
        "str_pad",
        "str_repeat",
        "str_replace",
        "str_split",
        "str_starts_with",
        "str_word_count",
        "strcasecmp",
        "strcmp",
        "strip_tags",
        "stripslashes",
        "stristr",
        "strlen",
        "strncasecmp",
        "strncmp",
        "strpos",
        "strrpos",
        "strstr",
        "strtolower",
        "strtotime",
        "strtoupper",
        "strval",
        "substr",
        "substr_count",
        "substr_replace",
        "tan",
        "time",
        "trim",
        "uasort",
        "ucfirst",
        "ucwords",
        "uksort",
        "unlink",
        "unserialize",
        "unset",
        "urldecode",
        "urlencode",
        "usleep",
        "usort",
        "var_dump",
        "var_export",
        "vsprintf",
    ];
    debug_assert!(
        BUILTINS.windows(2).all(|w| w[0] <= w[1]),
        "BUILTINS must be sorted for binary_search"
    );
    BUILTINS.binary_search(&name).is_ok()
}

/// Build the php.net documentation URL for a built-in function name.
pub(crate) fn php_doc_url(name: &str) -> String {
    // php.net uses underscores replaced with dashes in the URL path.
    let slug = name.replace('_', "-");
    format!("https://www.php.net/function.{}", slug)
}

/// Convert a UTF-16 code unit offset into a UTF-8 byte offset for `s`.
///
/// LSP positions use UTF-16 code units; Rust strings are UTF-8.  This helper
/// walks the string's `char_indices`, accumulating UTF-16 units, and returns
/// the byte index of the character at the given UTF-16 offset.  If the offset
/// is past the end of the string, `s.len()` is returned.
pub(crate) fn utf16_offset_to_byte(s: &str, utf16_offset: usize) -> usize {
    let mut utf16_count = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        if utf16_count >= utf16_offset {
            return byte_idx;
        }
        utf16_count += ch.len_utf16();
    }
    s.len()
}

/// Convert a UTF-8 byte offset into a UTF-16 code unit count.
///
/// LSP `Position.character` is measured in UTF-16 code units.  Given a string
/// and a byte offset into it, this returns how many UTF-16 units precede that
/// offset — which is the correct LSP character value.
pub(crate) fn byte_to_utf16(s: &str, byte_offset: usize) -> u32 {
    s[..byte_offset.min(s.len())]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum()
}

/// Split a parameter list string on commas, respecting bracket nesting.
///
/// This avoids splitting inside default values like `array $x = [1, 2, 3]`.
/// Each returned slice is trimmed of leading/trailing whitespace.
pub(crate) fn split_params(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
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
pub(crate) fn word_at(source: &str, position: Position) -> Option<String> {
    // Use split('\n') rather than lines() so that a trailing newline produces a
    // final empty entry — lines() silently drops it, causing word_at to return
    // None for any cursor on the last line of a normally-saved PHP file.
    let raw = source.split('\n').nth(position.line as usize)?;
    let line = raw.strip_suffix('\r').unwrap_or(raw);
    let char_offset = position.character as usize;

    let chars: Vec<char> = line.chars().collect();

    let mut utf16_len = 0usize;
    let mut char_pos = 0usize;
    for ch in &chars {
        if utf16_len >= char_offset {
            break;
        }
        utf16_len += ch.len_utf16();
        char_pos += 1;
    }

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
        return None;
    }

    let word: String = chars[left..right].iter().collect();
    if word.is_empty() { None } else { Some(word) }
}

/// Extract the source text covered by an LSP `Range`.
///
/// `Range` positions use UTF-16 code-unit offsets; this function converts them
/// correctly before slicing the UTF-8 source string.
pub(crate) fn selected_text_range(source: &str, range: tower_lsp::lsp_types::Range) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_to_utf16_ascii() {
        assert_eq!(byte_to_utf16("hello", 3), 3);
    }

    #[test]
    fn byte_to_utf16_multibyte_bmp() {
        // "é" is U+00E9: 2 bytes in UTF-8, 1 code unit in UTF-16.
        let s = "café";
        assert_eq!(byte_to_utf16(s, 0), 0);
        assert_eq!(byte_to_utf16(s, 3), 3); // up to "caf" (all ASCII)
        assert_eq!(byte_to_utf16(s, 5), 4); // full string (é = 2 bytes → 1 UTF-16 unit)
    }

    #[test]
    fn byte_to_utf16_surrogate_pair() {
        // "😀" is U+1F600: 4 bytes in UTF-8, 2 code units in UTF-16 (surrogate pair).
        let s = "a😀b";
        assert_eq!(byte_to_utf16(s, 1), 1); // after "a"
        assert_eq!(byte_to_utf16(s, 5), 3); // after "a😀" (emoji = 4 bytes → 2 UTF-16 units)
        assert_eq!(byte_to_utf16(s, 6), 4); // full string
    }

    #[test]
    fn byte_to_utf16_past_end_clamps() {
        assert_eq!(byte_to_utf16("hi", 100), 2);
    }

    #[test]
    fn utf16_offset_to_byte_ascii() {
        assert_eq!(utf16_offset_to_byte("hello", 3), 3);
    }

    #[test]
    fn utf16_offset_to_byte_surrogate_pair() {
        // "a😀b": UTF-16 offset 1 → byte 1 (start of emoji), offset 3 → byte 5 (after emoji)
        let s = "a😀b";
        assert_eq!(utf16_offset_to_byte(s, 1), 1);
        assert_eq!(utf16_offset_to_byte(s, 3), 5);
    }

    #[test]
    fn byte_to_utf16_and_back_roundtrip() {
        let s = "café 😀 world";
        for (byte_idx, _) in s.char_indices() {
            let utf16 = byte_to_utf16(s, byte_idx) as usize;
            assert_eq!(utf16_offset_to_byte(s, utf16), byte_idx);
        }
    }

    #[test]
    fn word_at_last_line_with_trailing_newline() {
        // Editors save files with a trailing newline; lines() drops the final
        // empty entry, making word_at return None for cursors on the last line.
        let src = "<?php\necho strlen($x);\n";
        let pos = Position {
            line: 1,
            character: 6,
        }; // "strlen" on line 1
        let w = word_at(src, pos);
        assert_eq!(
            w.as_deref(),
            Some("strlen"),
            "word_at must work on lines before the trailing newline"
        );
        // Position on the final empty line produced by the trailing newline.
        let last_line = Position {
            line: 2,
            character: 0,
        };
        // Should return None (empty line), but must not panic.
        let _ = word_at(src, last_line);
    }

    #[test]
    fn word_at_crlf_line_endings() {
        let src = "<?php\r\nfunction foo() {}\r\n";
        let pos = Position {
            line: 1,
            character: 9,
        }; // "foo"
        let w = word_at(src, pos);
        assert_eq!(
            w.as_deref(),
            Some("foo"),
            "word_at must handle CRLF line endings"
        );
    }

    #[test]
    fn is_php_builtin_asin_recognized() {
        // asin was out of order in BUILTINS, causing binary_search to miss it.
        assert!(
            is_php_builtin("asin"),
            "asin must be recognised as a PHP builtin"
        );
        assert!(
            is_php_builtin("atan"),
            "atan must be recognised as a PHP builtin"
        );
        assert!(
            is_php_builtin("krsort"),
            "krsort must be recognised as a PHP builtin"
        );
        assert!(
            is_php_builtin("strcasecmp"),
            "strcasecmp must be recognised as a PHP builtin"
        );
        assert!(
            is_php_builtin("strncasecmp"),
            "strncasecmp must be recognised as a PHP builtin"
        );
        assert!(
            is_php_builtin("strip_tags"),
            "strip_tags must be recognised as a PHP builtin"
        );
    }
}
