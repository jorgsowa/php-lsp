use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation,
};

use crate::ast::ParsedDoc;
use crate::hover::format_params_str;

/// Returns signature help for the function call the cursor is inside of.
pub fn signature_help(source: &str, doc: &ParsedDoc, position: Position) -> Option<SignatureHelp> {
    let (func_name, active_param) = call_context(source, position)?;
    let sig_text = find_signature(&doc.program().stmts, &func_name)
        .or_else(|| builtin_signature(&func_name).map(|s| s.to_string()))?;

    let label = format!("{}({})", func_name, sig_text);
    let params: Vec<ParameterInformation> = sig_text
        .split(',')
        .map(|p| ParameterInformation {
            label: ParameterLabel::Simple(p.trim().to_string()),
            documentation: None,
        })
        .filter(|p| {
            if let ParameterLabel::Simple(s) = &p.label {
                !s.is_empty()
            } else {
                true
            }
        })
        .collect();

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: if params.is_empty() {
                None
            } else {
                Some(params)
            },
            active_parameter: Some(active_param as u32),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param as u32),
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
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            return Some(format_params_str(&m.params));
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(s) = find_signature(inner, word) {
                        return Some(s);
                    }
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
    ("array_keys", "$array, $filter_value = null, $strict = false"),
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
    ("array_slice", "$array, $offset, $length = null, $preserve_keys = false"),
    ("array_splice", "&$array, $offset, $length = null, $replacement = []"),
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
    ("chunk_split", "$string, $length = 76, $separator = \"\\r\\n\""),
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
    ("extract", "&$array, $flags = EXTR_OVERWRITE, $prefix = null"),
    ("fclose", "$handle"),
    ("feof", "$handle"),
    ("fgets", "$handle, $length = null"),
    ("file_exists", "$filename"),
    ("file_get_contents", "$filename, $use_include_path = false, $context = null, $offset = 0, $length = null"),
    ("file_put_contents", "$filename, $data, $flags = 0, $context = null"),
    ("floatval", "$value"),
    ("floor", "$num"),
    ("fmod", "$num1, $num2"),
    ("fopen", "$filename, $mode, $use_include_path = false, $context = null"),
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
    ("htmlspecialchars", "$string, $flags = ENT_QUOTES|ENT_SUBSTITUTE, $encoding = 'UTF-8', $double_encode = true"),
    ("htmlspecialchars_decode", "$string, $flags = ENT_QUOTES|ENT_SUBSTITUTE"),
    ("implode", "$separator, $array"),
    ("in_array", "$needle, $haystack, $strict = false"),
    ("intdiv", "$num, $divisor"),
    ("interface_exists", "$interface, $autoload = true"),
    ("intval", "$value, $base = 10"),
    ("is_a", "$object_or_class, $class_name, $allow_string = false"),
    ("is_array", "$value"),
    ("is_bool", "$value"),
    ("is_callable", "$value, $syntax_only = false, &$callable_name = null"),
    ("is_dir", "$filename"),
    ("is_file", "$filename"),
    ("is_float", "$value"),
    ("is_int", "$value"),
    ("is_null", "$value"),
    ("is_numeric", "$value"),
    ("is_object", "$value"),
    ("is_string", "$value"),
    ("isset", "$var, ...$vars"),
    ("json_decode", "$json, $associative = null, $depth = 512, $flags = 0"),
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
    ("mkdir", "$directory, $permissions = 0777, $recursive = false, $context = null"),
    ("mktime", "$hour, $minute, $second, $month, $day, $year"),
    ("mt_rand", "$min = 0, $max = mt_getrandmax()"),
    ("nl2br", "$string, $use_xhtml = true"),
    ("number_format", "$num, $decimals = 0, $decimal_separator = '.', $thousands_separator = ','"),
    ("ob_end_clean", ""),
    ("ob_get_clean", ""),
    ("ob_start", "$callback = null, $chunk_size = 0, $flags = PHP_OUTPUT_HANDLER_STDFLAGS"),
    ("phpversion", "$extension = null"),
    ("pow", "$base, $exp"),
    ("preg_match", "$pattern, $subject, &$matches = null, $flags = 0, $offset = 0"),
    ("preg_match_all", "$pattern, $subject, &$matches = null, $flags = PREG_PATTERN_ORDER, $offset = 0"),
    ("preg_quote", "$string, $delimiter = null"),
    ("preg_replace", "$pattern, $replacement, $subject, $limit = -1, &$count = null"),
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
    ("scandir", "$directory, $sorting_order = SCANDIR_SORT_ASCENDING, $context = null"),
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
    ("str_pad", "$string, $length, $pad_string = ' ', $pad_type = STR_PAD_RIGHT"),
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
    ("substr_count", "$haystack, $needle, $offset = 0, $length = null"),
    ("substr_replace", "$string, $replace, $offset, $length = null"),
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
    ("wordwrap", "$string, $width = 75, $break = \"\\n\", $cut_long_words = false"),
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
}
