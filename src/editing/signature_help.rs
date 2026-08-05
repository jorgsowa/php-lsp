use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::{
    Documentation, MarkupContent, MarkupKind, ParameterInformation, ParameterLabel, Position,
    SignatureHelp, SignatureInformation,
};

use crate::analysis::callable_info::callable_info_for_name;
use crate::document::ast::ParsedDoc;
use crate::hover::format_params_str;
use crate::lang::docblock::{find_docblock, parse_docblock};
use crate::text::{fqn_short_name, split_params, utf16_offset_to_byte};

/// Returns signature help for the function call the cursor is inside of.
///
/// Uses the current file's AST for same-file declarations (preserving exact
/// default-value text), then falls back to mir-backed callable resolution for
/// cross-file functions/methods/constructors.
pub fn signature_help(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    session: Option<&mir_analyzer::AnalysisSession>,
) -> Option<SignatureHelp> {
    let ctx = call_context(source, position)?;
    let func_name = ctx.name.clone();
    let active_param = ctx.active_param;
    let receiver = ctx.receiver.clone();
    let explicit_class_receiver = receiver
        .as_deref()
        .is_some_and(|r| !r.starts_with('$') && r != "self" && r != "static");

    let local_sig = (!explicit_class_receiver)
        .then(|| find_signature(&doc.program().stmts, &func_name, receiver.is_some()))
        .flatten();
    let local_doc_method_sig = receiver.as_deref().and_then(|recv| {
        let class_name = if recv == "$this" || recv == "self" || recv == "static" {
            crate::types::type_map::enclosing_class_at(source, doc, position).or_else(|| {
                analysis.and_then(|a| {
                    receiver_var_offset(source, doc, position, "$this")
                        .and_then(|off| crate::types::type_query::type_at_offset(a, off))
                        .and_then(crate::types::type_query::primary_class_name)
                })
            })
        } else if recv == "parent" {
            crate::types::type_map::enclosing_class_at(source, doc, position)
                .and_then(|fqcn| parent_class_in_doc(&doc.program().stmts, &fqcn))
        } else if recv.starts_with('$') {
            analysis.and_then(|a| {
                receiver_var_offset(source, doc, position, recv)
                    .and_then(|off| crate::types::type_query::type_at_offset(a, off))
                    .and_then(crate::types::type_query::primary_class_name)
            })
        } else {
            Some(recv.to_string())
        }?;
        find_doc_method_params_in_doc(&doc.program().stmts, &class_name, &func_name)
    });
    let resolved = session.and_then(|session| {
        analysis
            .and_then(|a| a.symbol_at(ctx.name_byte_offset))
            .and_then(|symbol| symbol.to_symbol())
            .and_then(|symbol| callable_info_for_name(session, &symbol))
    });
    let sig_text = local_sig
        .or(local_doc_method_sig)
        .or_else(|| resolved.as_ref().map(|info| info.params.clone()))
        .or_else(|| builtin_signature(&func_name).map(|s| s.to_string()))?;

    let display_name = func_name.trim_start_matches('\\');
    let label = format!("{}({})", display_name, sig_text);
    let docblock = find_docblock(&doc.program().stmts, &func_name);
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

    let sig_doc = docblock
        .as_ref()
        .filter(|db| !db.description.is_empty())
        .map(|db| db.description.clone())
        .or_else(|| {
            resolved
                .as_ref()
                .and_then(|info| info.documentation.clone())
                .filter(|s| !s.is_empty())
        })
        .map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        });

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: sig_doc,
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

/// Scan backward from the cursor to find the enclosing function call name,
/// the index of the current parameter (0-based comma count), and — for method
/// calls — the receiver token (e.g. `"$this"`, `"$obj"`, `"ClassName"`).
struct CallContext {
    name: String,
    active_param: usize,
    receiver: Option<String>,
    name_byte_offset: u32,
}

fn call_context(source: &str, position: Position) -> Option<CallContext> {
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
    let in_string = string_literal_mask(&text);
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut i = text.len();

    while i > 0 {
        i -= 1;
        if in_string[i] {
            continue;
        }
        match text[i] {
            ')' | ']' => depth += 1,
            '(' | '[' if depth > 0 => depth -= 1,
            '(' if depth == 0 => {
                if let Some((name, name_start)) = extract_name_before(&text, i) {
                    let receiver = extract_receiver_before(&text, i, name.chars().count());
                    return Some(CallContext {
                        name,
                        active_param: commas,
                        receiver,
                        name_byte_offset: text[..name_start]
                            .iter()
                            .collect::<String>()
                            .len() as u32,
                    });
                }
                return None;
            }
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

/// Marks which positions in `text` fall inside a string literal or comment,
/// so the backward scan in `call_context` doesn't miscount parens or commas
/// that happen to appear inside an argument string (or a docblock — e.g. an
/// apostrophe in "the user's name" must not be mistaken for the start of an
/// unterminated `'` string). Mirrors `completion::cursor_in_string_or_comment`
/// but as a full forward-scan mask instead of a single point-query. Heredoc/
/// nowdoc are not tracked, matching that function's documented tradeoff.
pub(crate) fn string_literal_mask(text: &[char]) -> Vec<bool> {
    enum S {
        Normal,
        Single,
        Double,
        Line,
        Block,
    }
    let len = text.len();
    let mut mask = vec![false; len];
    let mut state = S::Normal;
    let mut i = 0usize;
    while i < len {
        match state {
            S::Normal => match text[i] {
                '\'' => {
                    mask[i] = true;
                    state = S::Single;
                    i += 1;
                }
                '"' => {
                    mask[i] = true;
                    state = S::Double;
                    i += 1;
                }
                '/' if i + 1 < len && text[i + 1] == '/' => {
                    mask[i] = true;
                    mask[i + 1] = true;
                    state = S::Line;
                    i += 2;
                }
                // `#[` is a PHP 8 attribute — not a comment.
                '#' if !(i + 1 < len && text[i + 1] == '[') => {
                    mask[i] = true;
                    state = S::Line;
                    i += 1;
                }
                '/' if i + 1 < len && text[i + 1] == '*' => {
                    mask[i] = true;
                    mask[i + 1] = true;
                    state = S::Block;
                    i += 2;
                }
                _ => i += 1,
            },
            S::Single => {
                mask[i] = true;
                match text[i] {
                    '\\' => {
                        if i + 1 < len {
                            mask[i + 1] = true;
                        }
                        i += 2;
                    }
                    '\'' => {
                        state = S::Normal;
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            S::Double => {
                mask[i] = true;
                match text[i] {
                    '\\' => {
                        if i + 1 < len {
                            mask[i + 1] = true;
                        }
                        i += 2;
                    }
                    '"' => {
                        state = S::Normal;
                        i += 1;
                    }
                    _ => i += 1,
                }
            }
            S::Line => {
                mask[i] = true;
                if text[i] == '\n' {
                    state = S::Normal;
                }
                i += 1;
            }
            S::Block => {
                if text[i] == '*' && i + 1 < len && text[i + 1] == '/' {
                    mask[i] = true;
                    mask[i + 1] = true;
                    state = S::Normal;
                    i += 2;
                } else {
                    mask[i] = true;
                    i += 1;
                }
            }
        }
    }
    mask
}

/// Byte offset of the last char of `receiver_var` in the nearest
/// `receiver_var->` / `receiver_var?->` / `receiver_var::` occurrence before
/// the cursor — a position inside mir's end-exclusive variable span. Mirrors
/// `hover/named_args.rs::receiver_var_offset`.
fn receiver_var_offset(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    receiver_var: &str,
) -> Option<u32> {
    let line = source.lines().nth(position.line as usize)?;
    let cursor_byte = utf16_offset_to_byte(line, position.character as usize).min(line.len());
    let before = &line[..cursor_byte];
    let p = before
        .rfind(&format!("{receiver_var}?->"))
        .or_else(|| before.rfind(&format!("{receiver_var}->")))
        .or_else(|| before.rfind(&format!("{receiver_var}::")))?;
    let line_start = doc.view().byte_of_position(Position {
        line: position.line,
        character: 0,
    });
    Some(line_start + (p + receiver_var.len()) as u32 - 1)
}

/// Extract the receiver token that precedes `->` or `::` just before the
/// method name at `text[name_start..paren_pos]`.  Returns `None` for plain
/// function calls (no arrow/double-colon operator before the name).
fn extract_receiver_before(text: &[char], paren_pos: usize, name_len: usize) -> Option<String> {
    let name_start = paren_pos.checked_sub(name_len)?;
    // skip any spaces between receiver operator and method name
    let mut end = name_start;
    while end > 0 && text[end - 1] == ' ' {
        end -= 1;
    }
    if end < 2 {
        return None;
    }
    let is_arrow = text[end - 2] == '-' && text[end - 1] == '>';
    let is_static = text[end - 2] == ':' && text[end - 1] == ':';
    if !is_arrow && !is_static {
        return None;
    }
    // Nullsafe `?->` — skip the `?` too so the receiver scan doesn't stop dead.
    let recv_end = if is_arrow && end >= 3 && text[end - 3] == '?' {
        end - 3
    } else {
        end - 2
    };
    let is_recv_char = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut recv_start = recv_end;
    while recv_start > 0 && is_recv_char(text[recv_start - 1]) {
        recv_start -= 1;
    }
    if recv_start == recv_end {
        return None;
    }
    Some(text[recv_start..recv_end].iter().collect())
}

fn extract_name_before(text: &[char], paren_pos: usize) -> Option<(String, usize)> {
    if paren_pos == 0 {
        return None;
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
        return None;
    }
    Some((text[start..end].iter().collect(), start))
}

fn parent_class_in_doc(stmts: &[Stmt<'_, '_>], fqcn: &str) -> Option<String> {
    parent_class_in_doc_impl(stmts, fqn_short_name(fqcn))
}

fn parent_class_in_doc_impl(stmts: &[Stmt<'_, '_>], class_name: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name.as_ref().and_then(|n| n.as_str()) == Some(class_name) => {
                return c.extends.as_ref().map(|p| p.to_string_repr().into_owned());
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(parent) = parent_class_in_doc_impl(&inner.stmts, class_name)
                {
                    return Some(parent);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_doc_method_params_in_doc(
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    method_name: &str,
) -> Option<String> {
    find_doc_method_params_in_doc_impl(stmts, fqn_short_name(class_name), method_name)
}

fn find_doc_method_params_in_doc_impl(
    stmts: &[Stmt<'_, '_>],
    class_name: &str,
    method_name: &str,
) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name.as_ref().and_then(|n| n.as_str()) == Some(class_name) => {
                let method = parse_docblock(c.doc_comment?.text)
                    .methods
                    .into_iter()
                    .find(|m| m.name.eq_ignore_ascii_case(method_name))?;
                return Some(
                    method
                        .params
                        .iter()
                        .map(|p| {
                            let mut s = String::new();
                            if !p.type_hint.is_empty() {
                                s.push_str(&p.type_hint);
                                s.push(' ');
                            }
                            if p.is_byref {
                                s.push('&');
                            }
                            if p.is_variadic {
                                s.push_str("...");
                            }
                            s.push('$');
                            s.push_str(&p.name);
                            if p.is_optional {
                                s.push_str(" = ...");
                            }
                            s
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(params) =
                        find_doc_method_params_in_doc_impl(&inner.stmts, class_name, method_name)
                {
                    return Some(params);
                }
            }
            _ => {}
        }
    }
    None
}

/// `has_receiver` gates class/interface/trait/enum member matching: a bare
/// call (no `->`/`::`/`?->`) can only ever reach a top-level `function`
/// declaration (or a class name used as `new ClassName(...)`), never a
/// member of the same name, since PHP has no syntax to invoke a method
/// without a receiver.
fn find_signature(stmts: &[Stmt<'_, '_>], word: &str, has_receiver: bool) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                return Some(format_params_str(&f.params));
            }
            StmtKind::Class(c) => {
                if has_receiver {
                    for member in c.body.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name.or_error().eq_ignore_ascii_case(word)
                        {
                            return Some(format_params_str(&m.params));
                        }
                    }
                }
                if c.name.as_ref().map(|n| n.to_string()) == Some(word.to_string()) {
                    for member in c.body.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind
                            && m.name == "__construct"
                        {
                            return Some(format_params_str(&m.params));
                        }
                    }
                }
            }
            StmtKind::Interface(i) if has_receiver => {
                for member in i.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name.or_error().eq_ignore_ascii_case(word)
                    {
                        return Some(format_params_str(&m.params));
                    }
                }
            }
            StmtKind::Trait(t) if has_receiver => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name.or_error().eq_ignore_ascii_case(word)
                    {
                        return Some(format_params_str(&m.params));
                    }
                }
            }
            StmtKind::Enum(e) if has_receiver => {
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && m.name.or_error().eq_ignore_ascii_case(word)
                    {
                        return Some(format_params_str(&m.params));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = find_signature(&inner.stmts, word, has_receiver)
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
    let lookup = name.trim_start_matches('\\');
    BUILTIN_SIGS
        .binary_search_by_key(&lookup, |&(n, _)| n)
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
