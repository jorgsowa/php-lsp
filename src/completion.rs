use std::sync::Arc;

use php_ast::{ClassMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

use crate::ast::ParsedDoc;
use crate::type_map::{enclosing_class_at, members_of_class, params_of_function, TypeMap};

const PHP_KEYWORDS: &[&str] = &[
    "abstract", "and", "array", "as", "break", "callable", "case", "catch", "class", "clone",
    "const", "continue", "declare", "default", "die", "do", "echo", "else", "elseif", "empty",
    "enddeclare", "endfor", "endforeach", "endif", "endswitch", "endwhile", "enum", "eval",
    "exit", "extends", "final", "finally", "fn", "for", "foreach", "function", "global", "goto",
    "if", "implements", "include", "include_once", "instanceof", "insteadof", "interface",
    "isset", "list", "match", "namespace", "new", "null", "or", "print", "private", "protected",
    "public", "readonly", "require", "require_once", "return", "self", "static", "switch",
    "throw", "trait", "true", "false", "try", "use", "var", "while", "xor", "yield",
];

pub fn keyword_completions() -> Vec<CompletionItem> {
    PHP_KEYWORDS
        .iter()
        .map(|kw| CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect()
}

pub fn symbol_completions(doc: &ParsedDoc) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    collect_from_statements(&doc.program().stmts, &mut items);
    items
}

fn collect_from_statements(stmts: &[Stmt<'_, '_>], items: &mut Vec<CompletionItem>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                items.push(CompletionItem {
                    label: f.name.to_string(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    ..Default::default()
                });
                for param in f.params.iter() {
                    items.push(CompletionItem {
                        label: format!("${}", param.name),
                        kind: Some(CompletionItemKind::VARIABLE),
                        ..Default::default()
                    });
                }
            }
            StmtKind::Class(c) => {
                let class_name = c.name.unwrap_or("");
                if !class_name.is_empty() {
                    items.push(CompletionItem {
                        label: class_name.to_string(),
                        kind: Some(CompletionItemKind::CLASS),
                        ..Default::default()
                    });
                }
                for member in c.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            items.push(CompletionItem {
                                label: m.name.to_string(),
                                kind: Some(CompletionItemKind::METHOD),
                                ..Default::default()
                            });
                        }
                        ClassMemberKind::Property(p) => {
                            items.push(CompletionItem {
                                label: format!("${}", p.name),
                                kind: Some(CompletionItemKind::PROPERTY),
                                ..Default::default()
                            });
                        }
                        ClassMemberKind::ClassConst(c) => {
                            items.push(CompletionItem {
                                label: c.name.to_string(),
                                kind: Some(CompletionItemKind::CONSTANT),
                                ..Default::default()
                            });
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Interface(i) => {
                items.push(CompletionItem {
                    label: i.name.to_string(),
                    kind: Some(CompletionItemKind::INTERFACE),
                    ..Default::default()
                });
            }
            StmtKind::Trait(t) => {
                items.push(CompletionItem {
                    label: t.name.to_string(),
                    kind: Some(CompletionItemKind::CLASS),
                    ..Default::default()
                });
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_from_statements(inner, items);
                }
            }
            StmtKind::Expression(e) => {
                collect_from_expression(e, items);
            }
            _ => {}
        }
    }
}

fn collect_from_expression(expr: &php_ast::Expr<'_, '_>, items: &mut Vec<CompletionItem>) {
    if let ExprKind::Assign(assign) = &expr.kind {
        if let ExprKind::Variable(name) = &assign.target.kind {
            let label = format!("${}", name);
            if label != "$this" {
                items.push(CompletionItem {
                    label,
                    kind: Some(CompletionItemKind::VARIABLE),
                    ..Default::default()
                });
            }
        }
        collect_from_expression(assign.value, items);
    }
}

const PHP_BUILTINS: &[&str] = &[
    // string
    "strlen", "strpos", "strrpos", "substr", "str_replace", "str_contains", "str_starts_with",
    "str_ends_with", "str_split", "explode", "implode", "join", "trim", "ltrim", "rtrim",
    "strtolower", "strtoupper", "ucfirst", "lcfirst", "ucwords", "sprintf", "printf", "vsprintf",
    "number_format", "nl2br", "htmlspecialchars", "htmlentities", "strip_tags", "addslashes",
    "stripslashes", "str_pad", "str_repeat", "str_word_count", "strcmp", "strcasecmp",
    "strncmp", "strncasecmp", "substr_count", "substr_replace", "strstr", "stristr",
    "preg_match", "preg_match_all", "preg_replace", "preg_split", "preg_quote",
    "md5", "sha1", "hash", "base64_encode", "base64_decode", "urlencode", "urldecode",
    "rawurlencode", "rawurldecode", "http_build_query", "parse_str", "parse_url",
    // array
    "count", "array_key_exists", "in_array", "array_search", "array_merge", "array_replace",
    "array_push", "array_pop", "array_shift", "array_unshift", "array_splice", "array_slice",
    "array_chunk", "array_combine", "array_diff", "array_intersect", "array_unique",
    "array_flip", "array_reverse", "array_keys", "array_values", "array_map", "array_filter",
    "array_reduce", "array_walk", "array_fill", "array_fill_keys", "array_pad",
    "sort", "rsort", "asort", "arsort", "ksort", "krsort", "usort", "uasort", "uksort",
    "compact", "extract", "list", "range",
    // math
    "abs", "ceil", "floor", "round", "max", "min", "pow", "sqrt", "log", "exp",
    "rand", "mt_rand", "random_int", "fmod", "intdiv", "intval", "floatval", "is_nan",
    "is_infinite", "is_finite", "pi", "sin", "cos", "tan", "asin", "acos", "atan", "atan2",
    // type / var
    "isset", "empty", "unset", "is_null", "is_bool", "is_int", "is_integer", "is_long",
    "is_float", "is_double", "is_string", "is_array", "is_object", "is_callable", "is_numeric",
    "is_a", "instanceof", "gettype", "settype", "intval", "floatval", "strval", "boolval",
    "var_dump", "var_export", "print_r", "serialize", "unserialize",
    // file / io
    "file_get_contents", "file_put_contents", "file_exists", "is_file", "is_dir", "is_readable",
    "is_writable", "mkdir", "rmdir", "unlink", "rename", "copy", "realpath", "dirname",
    "basename", "pathinfo", "glob", "scandir", "opendir", "readdir", "closedir",
    "fopen", "fclose", "fread", "fwrite", "fgets", "fputs", "feof", "fseek", "ftell", "rewind",
    // date / time
    "time", "microtime", "mktime", "strtotime", "date", "date_create", "date_format",
    "date_diff", "date_add", "date_sub", "checkdate",
    // misc
    "defined", "define", "constant", "class_exists", "interface_exists", "function_exists",
    "method_exists", "property_exists", "get_class", "get_parent_class", "is_subclass_of",
    "header", "headers_sent", "setcookie", "session_start", "session_destroy",
    "ob_start", "ob_get_clean", "ob_end_clean", "json_encode", "json_decode",
    "call_user_func", "call_user_func_array", "array_walk_recursive", "array_map",
    "compact", "extract", "sleep", "usleep", "exit", "die",
];

pub fn builtin_completions() -> Vec<CompletionItem> {
    let mut seen = std::collections::HashSet::new();
    PHP_BUILTINS
        .iter()
        .filter(|&&f| seen.insert(f))
        .map(|f| CompletionItem {
            label: f.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            ..Default::default()
        })
        .collect()
}

fn collect_instance_members(doc: &ParsedDoc, class_name: &str) -> Vec<CompletionItem> {
    let members = members_of_class(doc, class_name);
    let mut items = Vec::new();
    for (name, is_static) in members.methods {
        if !is_static {
            items.push(CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::METHOD),
                ..Default::default()
            });
        }
    }
    for (name, is_static) in members.properties {
        if !is_static {
            items.push(CompletionItem {
                label: format!("${name}"),
                kind: Some(CompletionItemKind::PROPERTY),
                ..Default::default()
            });
        }
    }
    items
}

fn collect_static_members(doc: &ParsedDoc, class_name: &str) -> Vec<CompletionItem> {
    let members = members_of_class(doc, class_name);
    let mut items = Vec::new();
    for (name, is_static) in members.methods {
        if is_static {
            items.push(CompletionItem {
                label: name,
                kind: Some(CompletionItemKind::METHOD),
                ..Default::default()
            });
        }
    }
    for (name, is_static) in members.properties {
        if is_static {
            items.push(CompletionItem {
                label: format!("${name}"),
                kind: Some(CompletionItemKind::PROPERTY),
                ..Default::default()
            });
        }
    }
    for name in members.constants {
        items.push(CompletionItem {
            label: name,
            kind: Some(CompletionItemKind::CONSTANT),
            ..Default::default()
        });
    }
    items
}

fn resolve_static_receiver(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = position.character as usize;
    let before = &line[..col.min(line.len())];
    let before = before.strip_suffix("::").unwrap_or(before);
    let name: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '\\')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if name.is_empty() { None } else { Some(name) }
}

fn resolve_call_params(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    position: Position,
) -> Vec<String> {
    let line = match source.lines().nth(position.line as usize) {
        Some(l) => l,
        None => return vec![],
    };
    let col = position.character as usize;
    let before = &line[..col.min(line.len())];
    let before = before.strip_suffix('(').unwrap_or(before);
    let func_name: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if func_name.is_empty() {
        return vec![];
    }
    let mut params = params_of_function(doc, &func_name);
    if params.is_empty() {
        for other in other_docs {
            params = params_of_function(other, &func_name);
            if !params.is_empty() {
                break;
            }
        }
    }
    params
}

pub fn filtered_completions(
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    trigger_character: Option<&str>,
) -> Vec<CompletionItem> {
    filtered_completions_at(doc, other_docs, trigger_character, None, None)
}

/// Like `filtered_completions` but also accepts an optional `source` + `position`
/// so that `->` completions can be scoped to the variable's class.
pub fn filtered_completions_at(
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    trigger_character: Option<&str>,
    source: Option<&str>,
    position: Option<Position>,
) -> Vec<CompletionItem> {
    match trigger_character {
        Some("$") => symbol_completions(doc)
            .into_iter()
            .filter(|i| i.kind == Some(CompletionItemKind::VARIABLE))
            .collect(),
        Some(">") => {
            // Arrow: $obj->  or  $this->
            if let (Some(src), Some(pos)) = (source, position) {
                let type_map = TypeMap::from_doc(doc);
                if let Some(class_name) = resolve_receiver_class(src, doc, pos, &type_map) {
                    let mut items = collect_instance_members(doc, &class_name);
                    for other in other_docs {
                        items.extend(collect_instance_members(other, &class_name));
                    }
                    let mut seen = std::collections::HashSet::new();
                    items.retain(|i| seen.insert(i.label.clone()));
                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            // Fallback: all methods from current doc
            symbol_completions(doc)
                .into_iter()
                .filter(|i| i.kind == Some(CompletionItemKind::METHOD))
                .collect()
        }
        Some(":") => {
            // Static access: ClassName::
            if let (Some(src), Some(pos)) = (source, position) {
                if let Some(class_name) = resolve_static_receiver(src, pos) {
                    let mut items = collect_static_members(doc, &class_name);
                    for other in other_docs {
                        items.extend(collect_static_members(other, &class_name));
                    }
                    let mut seen = std::collections::HashSet::new();
                    items.retain(|i| seen.insert(i.label.clone()));
                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            vec![]
        }
        Some("(") => {
            // Named argument: funcName(
            if let (Some(src), Some(pos)) = (source, position) {
                let params = resolve_call_params(src, doc, other_docs, pos);
                if !params.is_empty() {
                    return params
                        .into_iter()
                        .map(|p| CompletionItem {
                            label: format!("{p}:"),
                            kind: Some(CompletionItemKind::VARIABLE),
                            ..Default::default()
                        })
                        .collect();
                }
            }
            vec![]
        }
        _ => {
            let mut items = keyword_completions();
            items.extend(builtin_completions());
            items.extend(symbol_completions(doc));
            for other in other_docs {
                let cross: Vec<CompletionItem> = symbol_completions(other)
                    .into_iter()
                    .filter(|i| i.kind != Some(CompletionItemKind::VARIABLE))
                    .collect();
                items.extend(cross);
            }
            let mut seen = std::collections::HashSet::new();
            items.retain(|i| seen.insert(i.label.clone()));
            items
        }
    }
}

fn resolve_receiver_class(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    type_map: &TypeMap,
) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = position.character as usize;
    let before = &line[..col.min(line.len())];
    let before = before.strip_suffix("->").unwrap_or(before);
    let var_name: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '$')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if var_name.is_empty() {
        return None;
    }
    let var_name = if var_name.starts_with('$') {
        var_name
    } else {
        format!("${var_name}")
    };
    if var_name == "$this" {
        return enclosing_class_at(source, doc, position);
    }
    type_map.get(&var_name).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(source: &str) -> ParsedDoc {
        ParsedDoc::parse(source.to_string())
    }

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn keywords_list_is_non_empty() {
        assert!(!keyword_completions().is_empty());
    }

    #[test]
    fn keywords_contain_common_php_keywords() {
        let kws = keyword_completions();
        let ls = labels(&kws);
        for expected in &["function", "class", "return", "foreach", "match", "namespace"] {
            assert!(ls.contains(expected), "missing keyword: {expected}");
        }
    }

    #[test]
    fn all_keyword_items_have_keyword_kind() {
        for item in keyword_completions() {
            assert_eq!(item.kind, Some(CompletionItemKind::KEYWORD));
        }
    }

    #[test]
    fn extracts_top_level_function_name() {
        let d = doc("<?php\nfunction greet() {}");
        let items = symbol_completions(&d);
        assert!(labels(&items).contains(&"greet"));
        let greet = items.iter().find(|i| i.label == "greet").unwrap();
        assert_eq!(greet.kind, Some(CompletionItemKind::FUNCTION));
    }

    #[test]
    fn extracts_top_level_class_name() {
        let d = doc("<?php\nclass MyService {}");
        let items = symbol_completions(&d);
        assert!(labels(&items).contains(&"MyService"));
        let cls = items.iter().find(|i| i.label == "MyService").unwrap();
        assert_eq!(cls.kind, Some(CompletionItemKind::CLASS));
    }

    #[test]
    fn extracts_class_method_names() {
        let d = doc("<?php\nclass Calc { public function add() {} public function sub() {} }");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"add"), "missing 'add'");
        assert!(ls.contains(&"sub"), "missing 'sub'");
        for item in items.iter().filter(|i| i.label == "add" || i.label == "sub") {
            assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        }
    }

    #[test]
    fn extracts_function_parameters_as_variables() {
        let d = doc("<?php\nfunction process($input, $count) {}");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"$input"), "missing '$input'");
        assert!(ls.contains(&"$count"), "missing '$count'");
    }

    #[test]
    fn extracts_symbols_inside_namespace() {
        let d = doc("<?php\nnamespace App {\nfunction render() {}\nclass View {}\n}");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"render"), "missing 'render'");
        assert!(ls.contains(&"View"), "missing 'View'");
    }

    #[test]
    fn extracts_interface_name() {
        let d = doc("<?php\ninterface Serializable {}");
        let items = symbol_completions(&d);
        let item = items.iter().find(|i| i.label == "Serializable");
        assert!(item.is_some(), "missing 'Serializable'");
        assert_eq!(item.unwrap().kind, Some(CompletionItemKind::INTERFACE));
    }

    #[test]
    fn variable_assignment_produces_variable_item() {
        let d = doc("<?php\n$name = 'Alice';");
        let items = symbol_completions(&d);
        assert!(labels(&items).contains(&"$name"), "missing '$name'");
    }

    #[test]
    fn class_property_appears_in_completions() {
        let d = doc("<?php\nclass User { public string $name; private int $age; }");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"$name"), "missing '$name'");
        assert!(ls.contains(&"$age"), "missing '$age'");
        for item in items.iter().filter(|i| i.label == "$name" || i.label == "$age") {
            assert_eq!(item.kind, Some(CompletionItemKind::PROPERTY));
        }
    }

    #[test]
    fn class_constant_appears_in_completions() {
        let d = doc("<?php\nclass Status { const ACTIVE = 1; const INACTIVE = 0; }");
        let items = symbol_completions(&d);
        let ls = labels(&items);
        assert!(ls.contains(&"ACTIVE"), "missing 'ACTIVE'");
        assert!(ls.contains(&"INACTIVE"), "missing 'INACTIVE'");
    }

    #[test]
    fn dollar_trigger_returns_only_variables() {
        let d = doc("<?php\nfunction greet($name) {}\nclass Foo {}\n$bar = 1;");
        let items = filtered_completions(&d, &[], Some("$"));
        assert!(!items.is_empty(), "should have variable items");
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
        }
        let ls = labels(&items);
        assert!(!ls.contains(&"greet"), "should not contain function");
        assert!(!ls.contains(&"Foo"), "should not contain class");
    }

    #[test]
    fn arrow_trigger_returns_only_methods() {
        let d = doc("<?php\nclass Calc { public function add() {} public function sub() {} }");
        let items = filtered_completions(&d, &[], Some(">"));
        assert!(!items.is_empty(), "should have method items");
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        }
    }

    #[test]
    fn none_trigger_returns_keywords_functions_classes() {
        let d = doc("<?php\nfunction greet() {}\nclass MyApp {}");
        let items = filtered_completions(&d, &[], None);
        let ls = labels(&items);
        assert!(ls.contains(&"function"), "should contain keyword 'function'");
        assert!(ls.contains(&"greet"), "should contain function 'greet'");
        assert!(ls.contains(&"MyApp"), "should contain class 'MyApp'");
    }

    #[test]
    fn cross_file_symbols_appear_in_default_completions() {
        let d = doc("<?php\nfunction localFn() {}");
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nclass RemoteService {}\nfunction remoteHelper() {}".to_string(),
        ));
        let items = filtered_completions(&d, &[other], None);
        let ls = labels(&items);
        assert!(ls.contains(&"localFn"), "missing local function");
        assert!(ls.contains(&"RemoteService"), "missing cross-file class");
        assert!(ls.contains(&"remoteHelper"), "missing cross-file function");
    }

    #[test]
    fn cross_file_variables_not_included_in_default_completions() {
        let d = doc("<?php\n$localVar = 1;");
        let other = Arc::new(ParsedDoc::parse("<?php\n$remoteVar = 2;".to_string()));
        let items = filtered_completions(&d, &[other], None);
        let ls = labels(&items);
        assert!(!ls.contains(&"$remoteVar"), "cross-file variable should not appear");
    }
}
