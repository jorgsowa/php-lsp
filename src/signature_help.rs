use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    Documentation, ParameterInformation, ParameterLabel, Position, SignatureHelp,
    SignatureInformation,
};

use crate::ast::ParsedDoc;
use crate::docblock::find_docblock;
use crate::hover::format_params_str;
use crate::util::split_params;

/// Returns signature help for the function call the cursor is inside of.
pub fn signature_help(source: &str, doc: &ParsedDoc, position: Position) -> Option<SignatureHelp> {
    let (func_name, active_param) = call_context(source, position)?;
    let sig_text = find_signature(&doc.program().stmts, &func_name)
        .or_else(|| builtin_signature(&func_name).map(|s| s.to_string()))?;

    let label = format!("{}({})", func_name, sig_text);
    let docblock = find_docblock(source, &doc.program().stmts, &func_name);
    let params: Vec<ParameterInformation> = split_params(&sig_text)
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(|p| {
            // Extract the variable name (e.g. "$name") from the param string.
            let param_name = p
                .split_whitespace()
                .find(|t| t.starts_with('$'))
                .unwrap_or("")
                .trim_start_matches('$');
            let doc = docblock.as_ref().and_then(|db| {
                db.params
                    .iter()
                    .find(|dp| dp.name.trim_start_matches('$') == param_name)
                    .filter(|dp| !dp.description.is_empty())
                    .map(|dp| Documentation::String(dp.description.clone()))
            });
            ParameterInformation {
                label: ParameterLabel::Simple(p.to_string()),
                documentation: doc,
            }
        })
        .collect();

    // Cap the active parameter index so it never exceeds the declared parameter
    // array. This matters for variadic functions (where arg count > param count
    // is normal) and prevents clients from trying to highlight a non-existent
    // parameter slot.
    let n = params.len();
    let effective_active: Option<u32> = if n == 0 {
        None
    } else {
        Some(active_param.min(n - 1) as u32)
    };

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: if params.is_empty() {
                None
            } else {
                Some(params)
            },
            active_parameter: effective_active,
        }],
        active_signature: Some(0),
        active_parameter: effective_active,
    })
}

/// Scan backward from the cursor to find the enclosing function call name
/// and the index of the current parameter (0-based comma count).
fn call_context(source: &str, position: Position) -> Option<(String, usize)> {
    let mut chars_before = String::new();
    for (i, line) in source.lines().enumerate() {
        if i < position.line as usize {
            chars_before.push_str(line);
            chars_before.push('\n');
        } else if i == position.line as usize {
            let col = position.character as usize;
            let line_chars: Vec<char> = line.chars().collect();
            let mut utf16 = 0usize;
            let mut char_col = 0usize;
            for ch in &line_chars {
                if utf16 >= col {
                    break;
                }
                utf16 += ch.len_utf16();
                char_col += 1;
            }
            chars_before.extend(line_chars.iter().take(char_col));
            break;
        }
    }

    let text: Vec<char> = chars_before.chars().collect();
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut i = text.len();

    while i > 0 {
        i -= 1;
        match text[i] {
            ')' | ']' => depth += 1,
            '(' | '[' if depth > 0 => depth -= 1,
            '(' if depth == 0 => {
                let name = extract_name_before(&text, i);
                if !name.is_empty() {
                    return Some((name, commas));
                }
                return None;
            }
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

fn extract_name_before(text: &[char], paren_pos: usize) -> String {
    if paren_pos == 0 {
        return String::new();
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '\\';
    let mut end = paren_pos;
    while end > 0 && text[end - 1] == ' ' {
        end -= 1;
    }
    let mut start = end;
    while start > 0 && is_ident(text[start - 1]) {
        start -= 1;
    }
    if start == end {
        return String::new();
    }
    text[start..end].iter().collect()
}

fn find_signature(stmts: &[Stmt<'_, '_>], word: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(format_params_str(&f.params));
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == word
                    {
                        return Some(format_params_str(&m.params));
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == word
                    {
                        return Some(format_params_str(&m.params));
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name == word
                    {
                        return Some(format_params_str(&m.params));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = find_signature(inner, word)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

fn builtin_signature(name: &str) -> Option<&'static str> {
    BUILTIN_SIGS
        .binary_search_by_key(&name, |&(n, _)| n)
        .ok()
        .map(|i| BUILTIN_SIGS[i].1)
}

/// Sorted list of built-in PHP function signatures (name, params).
static BUILTIN_SIGS: &[(&str, &str)] = &[
    ("abs", "$num"),
    ("addslashes", "$string"),
    ("array_chunk", "$array, $length, $preserve_keys = false"),
    ("array_column", "$array, $column_key, $index_key = null"),
    ("array_combine", "$keys, $values"),
    ("array_count_values", "$array"),
    ("array_diff", "$array, ...$arrays"),
    ("array_fill", "$start_index, $count, $value"),
    ("array_fill_keys", "$keys, $value"),
    ("array_filter", "$array, $callback = null, $mode = 0"),
    ("array_flip", "$array"),
    ("array_intersect", "$array, ...$arrays"),
    ("array_is_list", "$array"),
    ("array_key_exists", "$key, $array"),
    ("array_key_first", "$array"),
    ("array_key_last", "$array"),
    (
        "array_keys",
        "$array, $filter_value = null, $strict = false",
    ),
    ("array_map", "$callback, $array, ...$arrays"),
    ("array_merge", "...$arrays"),
    ("array_merge_recursive", "...$arrays"),
    ("array_pad", "$array, $length, $value"),
    ("array_pop", "&$array"),
    ("array_push", "&$array, ...$values"),
    ("array_reduce", "$array, $callback, $initial = null"),
    ("array_reverse", "$array, $preserve_keys = false"),
    ("array_search", "$needle, $haystack, $strict = false"),
    ("array_shift", "&$array"),
    (
        "array_slice",
        "$array, $offset, $length = null, $preserve_keys = false",
    ),
    (
        "array_splice",
        "&$array, $offset, $length = null, $replacement = []",
    ),
    ("array_unique", "$array, $flags = SORT_STRING"),
    ("array_unshift", "&$array, ...$values"),
    ("array_values", "$array"),
    ("array_walk", "&$array, $callback, $arg = null"),
    ("arsort", "&$array, $flags = SORT_REGULAR"),
    ("asort", "&$array, $flags = SORT_REGULAR"),
    ("base64_decode", "$string, $strict = false"),
    ("base64_encode", "$string"),
    ("basename", "$path, $suffix = ''"),
    ("boolval", "$value"),
    ("call_user_func", "$callback, ...$args"),
    ("call_user_func_array", "$callback, $args"),
    ("ceil", "$num"),
    (
        "chunk_split",
        "$string, $length = 76, $separator = \"\\r\\n\"",
    ),
    ("class_exists", "$class, $autoload = true"),
    ("compact", "$var_names, ...$vars"),
    ("copy", "$from, $to, $context = null"),
    ("count", "$array, $mode = COUNT_NORMAL"),
    ("date", "$format, $timestamp = null"),
    ("dirname", "$path, $levels = 1"),
    ("empty", "$var"),
    ("error_reporting", "$error_level = null"),
    ("exp", "$num"),
    ("explode", "$separator, $string, $limit = PHP_INT_MAX"),
    (
        "extract",
        "&$array, $flags = EXTR_OVERWRITE, $prefix = null",
    ),
    ("fclose", "$handle"),
    ("feof", "$handle"),
    ("fgets", "$handle, $length = null"),
    ("file_exists", "$filename"),
    (
        "file_get_contents",
        "$filename, $use_include_path = false, $context = null, $offset = 0, $length = null",
    ),
    (
        "file_put_contents",
        "$filename, $data, $flags = 0, $context = null",
    ),
    ("floatval", "$value"),
    ("floor", "$num"),
    ("fmod", "$num1, $num2"),
    (
        "fopen",
        "$filename, $mode, $use_include_path = false, $context = null",
    ),
    ("fread", "$handle, $length"),
    ("function_exists", "$function"),
    ("fwrite", "$handle, $string, $length = null"),
    ("get_class", "$object = null"),
    ("get_parent_class", "$object_or_class = null"),
    ("gettype", "$value"),
    ("glob", "$pattern, $flags = 0"),
    ("hash", "$algo, $data, $binary = false"),
    ("header", "$header, $replace = true, $response_code = 0"),
    ("headers_sent", "&$filename = null, &$line = null"),
    (
        "htmlspecialchars",
        "$string, $flags = ENT_QUOTES|ENT_SUBSTITUTE, $encoding = 'UTF-8', $double_encode = true",
    ),
    (
        "htmlspecialchars_decode",
        "$string, $flags = ENT_QUOTES|ENT_SUBSTITUTE",
    ),
    ("implode", "$separator, $array"),
    ("in_array", "$needle, $haystack, $strict = false"),
    ("intdiv", "$num, $divisor"),
    ("interface_exists", "$interface, $autoload = true"),
    ("intval", "$value, $base = 10"),
    (
        "is_a",
        "$object_or_class, $class_name, $allow_string = false",
    ),
    ("is_array", "$value"),
    ("is_bool", "$value"),
    (
        "is_callable",
        "$value, $syntax_only = false, &$callable_name = null",
    ),
    ("is_dir", "$filename"),
    ("is_file", "$filename"),
    ("is_float", "$value"),
    ("is_int", "$value"),
    ("is_null", "$value"),
    ("is_numeric", "$value"),
    ("is_object", "$value"),
    ("is_string", "$value"),
    ("isset", "$var, ...$vars"),
    (
        "json_decode",
        "$json, $associative = null, $depth = 512, $flags = 0",
    ),
    ("json_encode", "$value, $flags = 0, $depth = 512"),
    ("krsort", "&$array, $flags = SORT_REGULAR"),
    ("ksort", "&$array, $flags = SORT_REGULAR"),
    ("lcfirst", "$string"),
    ("log", "$num, $base = M_E"),
    ("log10", "$num"),
    ("log2", "$num"),
    ("ltrim", "$string, $characters = \" \\t\\n\\r\\0\\x0B\""),
    ("max", "$value, ...$values"),
    ("md5", "$string, $binary = false"),
    ("method_exists", "$object_or_class, $method"),
    ("microtime", "$as_float = false"),
    ("min", "$value, ...$values"),
    (
        "mkdir",
        "$directory, $permissions = 0777, $recursive = false, $context = null",
    ),
    ("mktime", "$hour, $minute, $second, $month, $day, $year"),
    ("mt_rand", "$min = 0, $max = mt_getrandmax()"),
    ("nl2br", "$string, $use_xhtml = true"),
    (
        "number_format",
        "$num, $decimals = 0, $decimal_separator = '.', $thousands_separator = ','",
    ),
    ("ob_end_clean", ""),
    ("ob_get_clean", ""),
    (
        "ob_start",
        "$callback = null, $chunk_size = 0, $flags = PHP_OUTPUT_HANDLER_STDFLAGS",
    ),
    ("phpversion", "$extension = null"),
    ("pow", "$base, $exp"),
    (
        "preg_match",
        "$pattern, $subject, &$matches = null, $flags = 0, $offset = 0",
    ),
    (
        "preg_match_all",
        "$pattern, $subject, &$matches = null, $flags = PREG_PATTERN_ORDER, $offset = 0",
    ),
    ("preg_quote", "$string, $delimiter = null"),
    (
        "preg_replace",
        "$pattern, $replacement, $subject, $limit = -1, &$count = null",
    ),
    ("preg_split", "$pattern, $subject, $limit = -1, $flags = 0"),
    ("print_r", "$value, $return = false"),
    ("printf", "$format, ...$values"),
    ("property_exists", "$object_or_class, $property"),
    ("rand", "$min = 0, $max = getrandmax()"),
    ("random_int", "$min, $max"),
    ("rawurldecode", "$string"),
    ("rawurlencode", "$string"),
    ("realpath", "$path"),
    ("rename", "$from, $to, $context = null"),
    ("rmdir", "$directory, $context = null"),
    ("round", "$num, $precision = 0, $mode = PHP_ROUND_HALF_UP"),
    ("rsort", "&$array, $flags = SORT_REGULAR"),
    ("rtrim", "$string, $characters = \" \\t\\n\\r\\0\\x0B\""),
    (
        "scandir",
        "$directory, $sorting_order = SCANDIR_SORT_ASCENDING, $context = null",
    ),
    ("session_destroy", ""),
    ("session_start", "$options = []"),
    ("set_error_handler", "$callback, $error_levels = E_ALL"),
    ("settype", "&$var, $type"),
    ("sha1", "$string, $binary = false"),
    ("sleep", "$seconds"),
    ("sort", "&$array, $flags = SORT_REGULAR"),
    ("sprintf", "$format, ...$values"),
    ("sqrt", "$num"),
    ("str_contains", "$haystack, $needle"),
    ("str_ends_with", "$haystack, $needle"),
    (
        "str_pad",
        "$string, $length, $pad_string = ' ', $pad_type = STR_PAD_RIGHT",
    ),
    ("str_repeat", "$string, $times"),
    ("str_replace", "$search, $replace, $subject, &$count = null"),
    ("str_split", "$string, $length = 1"),
    ("str_starts_with", "$haystack, $needle"),
    ("str_word_count", "$string, $format = 0, $characters = null"),
    ("strcasecmp", "$string1, $string2"),
    ("strcmp", "$string1, $string2"),
    ("strip_tags", "$string, $allowed_tags = null"),
    ("stripslashes", "$string"),
    ("strlen", "$string"),
    ("strpos", "$haystack, $needle, $offset = 0"),
    ("strrpos", "$haystack, $needle, $offset = 0"),
    ("strtolower", "$string"),
    ("strtotime", "$datetime, $baseTimestamp = null"),
    ("strtoupper", "$string"),
    ("strval", "$value"),
    ("substr", "$string, $offset, $length = null"),
    (
        "substr_count",
        "$haystack, $needle, $offset = 0, $length = null",
    ),
    (
        "substr_replace",
        "$string, $replace, $offset, $length = null",
    ),
    ("time", ""),
    ("trigger_error", "$message, $error_level = E_USER_NOTICE"),
    ("trim", "$string, $characters = \" \\t\\n\\r\\0\\x0B\""),
    ("uasort", "&$array, $callback"),
    ("ucfirst", "$string"),
    ("ucwords", "$string, $separators = \" \\t\\r\\n\\f\\v\""),
    ("uksort", "&$array, $callback"),
    ("unlink", "$filename, $context = null"),
    ("unset", "$var, ...$vars"),
    ("urldecode", "$string"),
    ("urlencode", "$string"),
    ("usleep", "$microseconds"),
    ("usort", "&$array, $callback"),
    ("var_dump", "$value, ...$values"),
    ("var_export", "$value, $return = false"),
    (
        "wordwrap",
        "$string, $width = 75, $break = \"\\n\", $cut_long_words = false",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn returns_signature_for_known_function() {
        let src = "<?php\nfunction greet(string $name, int $times): void {}\ngreet(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(2, 6));
        assert!(result.is_some(), "expected signature help");
        let sh = result.unwrap();
        assert_eq!(sh.signatures[0].label, "greet(string $name, int $times)");
    }

    #[test]
    fn active_parameter_tracks_comma() {
        let src = "<?php\nfunction add(int $a, int $b): int {}\nadd($x, ";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(2, 8));
        assert!(result.is_some());
        let sh = result.unwrap();
        assert_eq!(
            sh.active_parameter,
            Some(1),
            "second param should be active"
        );
    }

    #[test]
    fn returns_none_outside_call() {
        let src = "<?php\nfunction greet() {}\n$x = 1;";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(2, 4));
        assert!(result.is_none());
    }

    #[test]
    fn returns_none_for_unknown_function() {
        let src = "<?php\nunknown(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(1, 8));
        assert!(
            result.is_none(),
            "unknown function should yield no signature"
        );
    }

    #[test]
    fn returns_signature_for_builtin_function() {
        let src = "<?php\nstrlen(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(1, 7));
        assert!(result.is_some(), "expected signature for strlen");
        let sh = result.unwrap();
        assert_eq!(sh.signatures[0].label, "strlen($string)");
    }

    #[test]
    fn default_values_shown_in_signature() {
        let src = "<?php\nfunction greet(string $name = 'World', int $times = 1): void {}\ngreet(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(2, 6));
        assert!(result.is_some(), "expected signature help");
        let sh = result.unwrap();
        let label = &sh.signatures[0].label;
        assert!(
            label.contains("= 'World'"),
            "signature should show default string value, got: {label}"
        );
        assert!(
            label.contains("= 1"),
            "signature should show default int value, got: {label}"
        );
    }

    #[test]
    fn builtin_sigs_are_sorted() {
        for w in BUILTIN_SIGS.windows(2) {
            assert!(
                w[0].0 <= w[1].0,
                "BUILTIN_SIGS out of order: {:?} >= {:?}",
                w[0].0,
                w[1].0
            );
        }
    }

    #[test]
    fn nested_call_shows_outer_signature() {
        // `outer(inner(` — with cursor right after the second `(` (col 12),
        // the innermost unclosed `(` belongs to `inner`, so `inner`'s signature
        // is returned by call_context (it scans backward to the first unmatched `(`).
        // With cursor at col 11 (before the second `(`), `outer` is the active call.
        let src = "<?php\nfunction outer(int $a, string $b): void {}\nfunction inner(float $x): int {}\nouter(inner(";
        let doc = ParsedDoc::parse(src.to_string());

        // Col 11 = inside `outer(inner` — the unmatched `(` belongs to `outer`.
        let result_outer = signature_help(src, &doc, pos(3, 11));
        let sh_outer = result_outer.expect("expected signature help for outer");
        assert_eq!(
            sh_outer.signatures[0].label, "outer(int $a, string $b)",
            "at col 11 the active call should be 'outer'"
        );

        // Col 12 = after `outer(inner(` — the unmatched `(` belongs to `inner`.
        let result_inner = signature_help(src, &doc, pos(3, 12));
        let sh_inner = result_inner.expect("expected signature help for inner");
        assert_eq!(
            sh_inner.signatures[0].label, "inner(float $x)",
            "at col 12 the active call should be 'inner'"
        );
    }

    #[test]
    fn trait_method_signature_is_found() {
        // Methods defined in traits should be found by signature_help.
        let src = "<?php\ntrait Logger {\n    public function log(string $msg, int $level): void {}\n}\nlog(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(4, 4));
        let sh = result.expect("expected signature help for trait method log");
        assert!(
            sh.signatures[0].label.contains("$msg"),
            "signature should contain '$msg', got: {}",
            sh.signatures[0].label
        );
    }

    #[test]
    fn enum_method_signature_is_found() {
        // Methods defined in enums should be found by signature_help.
        let src = "<?php\nenum Status {\n    public static function from(string $value): self {}\n}\nfrom(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(4, 5));
        let sh = result.expect("expected signature help for enum method from");
        assert!(
            sh.signatures[0].label.contains("$value"),
            "signature should contain '$value', got: {}",
            sh.signatures[0].label
        );
    }

    #[test]
    fn param_description_shown_in_parameter_info() {
        // @param descriptions from docblocks must survive the parse_docblock()
        // delegation to mir_analyzer and appear in signature-help parameter docs.
        let src = "<?php\n/**\n * @param string $name The user's name\n * @param int $times How many times to greet\n */\nfunction greet(string $name, int $times): void {}\ngreet(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(6, 6));
        let sh = result.expect("expected signature help");
        let params = sh.signatures[0]
            .parameters
            .as_ref()
            .expect("expected parameters");
        assert_eq!(params.len(), 2, "expected 2 parameters");
        let first_doc = params[0]
            .documentation
            .as_ref()
            .expect("first param should have documentation from @param description");
        assert!(
            matches!(first_doc, Documentation::String(s) if s.contains("user's name")),
            "@param description should be forwarded to parameter documentation, got: {:?}",
            first_doc
        );
    }

    #[test]
    fn method_call_signature_via_function_lookup() {
        // A method `process` defined in the current doc should be found by
        // signature_help when the cursor is inside `process(`.
        // (Note: the current implementation looks up by function/method name
        // without receiver-type resolution, so `process` matches the method.)
        let src = "<?php\nclass Worker {\n    public function process(string $job, int $priority): bool {}\n}\nprocess(";
        let doc = ParsedDoc::parse(src.to_string());
        let result = signature_help(src, &doc, pos(4, 8));
        let sh = result.expect("expected signature help for process");
        assert_eq!(
            sh.signatures[0].label, "process(string $job, int $priority)",
            "method signature should show all parameters"
        );
    }
}
