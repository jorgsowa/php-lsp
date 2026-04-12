use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

use crate::ast::{ParsedDoc, offset_to_position};

use super::{build_function_sig, callable_item, docblock_docs, named_arg_item};

pub fn symbol_completions(doc: &ParsedDoc) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    collect_from_statements_with_doc(&doc.program().stmts, &mut items, Some(doc));
    items
}

/// Like `symbol_completions` but only includes variables declared at or before `line`.
/// Non-variable items (functions, classes, etc.) are always included.
pub fn symbol_completions_before(doc: &ParsedDoc, line: u32) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    collect_from_statements_before(
        &doc.program().stmts,
        &mut items,
        line,
        doc.source(),
        Some(doc),
    );
    items
}

fn collect_from_statements_before(
    stmts: &[Stmt<'_, '_>],
    items: &mut Vec<CompletionItem>,
    line: u32,
    source: &str,
    doc: Option<&ParsedDoc>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Expression(e) => {
                // Only add variables if they appear at or before the cursor line
                let stmt_line = offset_to_position(source, stmt.span.start).line;
                if stmt_line <= line {
                    collect_from_expression(e, items);
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_from_statements_before(inner, items, line, source, doc);
                }
            }
            // Non-variable items: always include
            _ => {
                collect_from_statements_with_doc(std::slice::from_ref(stmt), items, doc);
            }
        }
    }
}

fn collect_from_statements_with_doc(
    stmts: &[Stmt<'_, '_>],
    items: &mut Vec<CompletionItem>,
    doc: Option<&ParsedDoc>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let sig = build_function_sig(f.name, &f.params, f.return_type.as_ref());
                let documentation = doc.and_then(|d| docblock_docs(d, f.name));
                let mut item =
                    callable_item(f.name, CompletionItemKind::FUNCTION, !f.params.is_empty());
                item.detail = Some(sig);
                item.documentation = documentation;
                items.push(item);
                if let Some(named) = named_arg_item(f.name, CompletionItemKind::FUNCTION, &f.params)
                {
                    items.push(named);
                }
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
                            let sig = build_function_sig(m.name, &m.params, m.return_type.as_ref());
                            let documentation = doc.and_then(|d| docblock_docs(d, m.name));
                            let mut item = callable_item(
                                m.name,
                                CompletionItemKind::METHOD,
                                !m.params.is_empty(),
                            );
                            item.detail = Some(sig);
                            item.documentation = documentation;
                            items.push(item);
                            if let Some(named) =
                                named_arg_item(m.name, CompletionItemKind::METHOD, &m.params)
                            {
                                items.push(named);
                            }
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
            StmtKind::Enum(e) => {
                items.push(CompletionItem {
                    label: e.name.to_string(),
                    kind: Some(CompletionItemKind::ENUM),
                    ..Default::default()
                });
                for member in e.members.iter() {
                    if let EnumMemberKind::Case(c) = &member.kind {
                        items.push(CompletionItem {
                            label: format!("{}::{}", e.name, c.name),
                            kind: Some(CompletionItemKind::ENUM_MEMBER),
                            ..Default::default()
                        });
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_from_statements_with_doc(inner, items, doc);
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
        match &assign.target.kind {
            ExprKind::Variable(name) => {
                let label = format!("${}", name.as_str());
                if label != "$this" {
                    items.push(CompletionItem {
                        label,
                        kind: Some(CompletionItemKind::VARIABLE),
                        ..Default::default()
                    });
                }
            }
            // Array destructuring: [$a, $b] = ... or list($a, $b) = ...
            ExprKind::Array(elements) => {
                for elem in elements.iter() {
                    if let ExprKind::Variable(name) = &elem.value.kind {
                        let label = format!("${}", name.as_str());
                        if label != "$this" {
                            items.push(CompletionItem {
                                label,
                                kind: Some(CompletionItemKind::VARIABLE),
                                ..Default::default()
                            });
                        }
                    }
                }
            }
            _ => {}
        }
        collect_from_expression(assign.value, items);
    }
}

const PHP_BUILTINS: &[&str] = &[
    // string
    "strlen",
    "strpos",
    "strrpos",
    "substr",
    "str_replace",
    "str_contains",
    "str_starts_with",
    "str_ends_with",
    "str_split",
    "explode",
    "implode",
    "join",
    "trim",
    "ltrim",
    "rtrim",
    "strtolower",
    "strtoupper",
    "ucfirst",
    "lcfirst",
    "ucwords",
    "sprintf",
    "printf",
    "vsprintf",
    "number_format",
    "nl2br",
    "htmlspecialchars",
    "htmlentities",
    "strip_tags",
    "addslashes",
    "stripslashes",
    "str_pad",
    "str_repeat",
    "str_word_count",
    "strcmp",
    "strcasecmp",
    "strncmp",
    "strncasecmp",
    "substr_count",
    "substr_replace",
    "strstr",
    "stristr",
    "preg_match",
    "preg_match_all",
    "preg_replace",
    "preg_split",
    "preg_quote",
    "md5",
    "sha1",
    "hash",
    "base64_encode",
    "base64_decode",
    "urlencode",
    "urldecode",
    "rawurlencode",
    "rawurldecode",
    "http_build_query",
    "parse_str",
    "parse_url",
    // array
    "count",
    "array_key_exists",
    "in_array",
    "array_search",
    "array_merge",
    "array_replace",
    "array_push",
    "array_pop",
    "array_shift",
    "array_unshift",
    "array_splice",
    "array_slice",
    "array_chunk",
    "array_combine",
    "array_diff",
    "array_intersect",
    "array_unique",
    "array_flip",
    "array_reverse",
    "array_keys",
    "array_values",
    "array_map",
    "array_filter",
    "array_reduce",
    "array_walk",
    "array_fill",
    "array_fill_keys",
    "array_pad",
    "sort",
    "rsort",
    "asort",
    "arsort",
    "ksort",
    "krsort",
    "usort",
    "uasort",
    "uksort",
    "compact",
    "extract",
    "list",
    "range",
    // math
    "abs",
    "ceil",
    "floor",
    "round",
    "max",
    "min",
    "pow",
    "sqrt",
    "log",
    "exp",
    "rand",
    "mt_rand",
    "random_int",
    "fmod",
    "intdiv",
    "intval",
    "floatval",
    "is_nan",
    "is_infinite",
    "is_finite",
    "pi",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    // type / var
    "isset",
    "empty",
    "unset",
    "is_null",
    "is_bool",
    "is_int",
    "is_integer",
    "is_long",
    "is_float",
    "is_double",
    "is_string",
    "is_array",
    "is_object",
    "is_callable",
    "is_numeric",
    "is_a",
    "instanceof",
    "gettype",
    "settype",
    "intval",
    "floatval",
    "strval",
    "boolval",
    "var_dump",
    "var_export",
    "print_r",
    "serialize",
    "unserialize",
    // file / io
    "file_get_contents",
    "file_put_contents",
    "file_exists",
    "is_file",
    "is_dir",
    "is_readable",
    "is_writable",
    "mkdir",
    "rmdir",
    "unlink",
    "rename",
    "copy",
    "realpath",
    "dirname",
    "basename",
    "pathinfo",
    "glob",
    "scandir",
    "opendir",
    "readdir",
    "closedir",
    "fopen",
    "fclose",
    "fread",
    "fwrite",
    "fgets",
    "fputs",
    "feof",
    "fseek",
    "ftell",
    "rewind",
    // date / time
    "time",
    "microtime",
    "mktime",
    "strtotime",
    "date",
    "date_create",
    "date_format",
    "date_diff",
    "date_add",
    "date_sub",
    "checkdate",
    // misc
    "defined",
    "define",
    "constant",
    "class_exists",
    "interface_exists",
    "function_exists",
    "method_exists",
    "property_exists",
    "get_class",
    "get_parent_class",
    "is_subclass_of",
    "header",
    "headers_sent",
    "setcookie",
    "session_start",
    "session_destroy",
    "ob_start",
    "ob_get_clean",
    "ob_end_clean",
    "json_encode",
    "json_decode",
    "call_user_func",
    "call_user_func_array",
    "array_walk_recursive",
    "array_map",
    "compact",
    "extract",
    "sleep",
    "usleep",
    "exit",
    "die",
];

pub fn builtin_completions() -> Vec<CompletionItem> {
    let mut seen = std::collections::HashSet::new();
    PHP_BUILTINS
        .iter()
        .filter(|&&f| seen.insert(f))
        .map(|f| callable_item(f, CompletionItemKind::FUNCTION, true))
        .collect()
}

const PHP_SUPERGLOBALS: &[&str] = &[
    "$_SERVER",
    "$_GET",
    "$_POST",
    "$_FILES",
    "$_COOKIE",
    "$_SESSION",
    "$_REQUEST",
    "$_ENV",
    "$GLOBALS",
];

pub fn superglobal_completions() -> Vec<CompletionItem> {
    PHP_SUPERGLOBALS
        .iter()
        .map(|&name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some("superglobal".to_string()),
            ..Default::default()
        })
        .collect()
}
