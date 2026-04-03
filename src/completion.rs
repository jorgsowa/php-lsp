use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, InsertTextFormat, Position, Range, TextEdit, Url,
};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::phpstorm_meta::PhpStormMeta;
use crate::stubs::builtin_class_members;
use crate::type_map::{
    TypeMap, enclosing_class_at, is_backed_enum, is_enum, members_of_class, mixin_classes_of,
    params_of_function, params_of_method, parent_class_name,
};
use crate::use_resolver::UseMap;
use crate::util::{camel_sort_key, fuzzy_camel_match, utf16_offset_to_byte};

/// Build a `CompletionItem` for a callable (function or method).
///
/// If the function has parameters the item uses snippet format with `$1`
/// inside the parentheses so the cursor lands there.  Zero-parameter
/// callables insert `name()` as plain text.
fn callable_item(label: &str, kind: CompletionItemKind, has_params: bool) -> CompletionItem {
    if has_params {
        CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            insert_text: Some(format!("{}($1)", label)),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }
    } else {
        CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            insert_text: Some(format!("{}()", label)),
            ..Default::default()
        }
    }
}

const PHP_KEYWORDS: &[&str] = &[
    "abstract",
    "and",
    "array",
    "as",
    "break",
    "callable",
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
    "insteadof",
    "interface",
    "isset",
    "list",
    "match",
    "namespace",
    "new",
    "null",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "self",
    "static",
    "switch",
    "throw",
    "trait",
    "true",
    "false",
    "try",
    "use",
    "var",
    "while",
    "xor",
    "yield",
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

const PHP_MAGIC_CONSTANTS: &[(&str, &str)] = &[
    ("__FILE__", "Absolute path of the current file"),
    ("__DIR__", "Directory of the current file"),
    ("__LINE__", "Current line number"),
    ("__CLASS__", "Current class name"),
    ("__FUNCTION__", "Current function name"),
    ("__METHOD__", "Current method name (Class::method)"),
    ("__NAMESPACE__", "Current namespace"),
    ("__TRAIT__", "Current trait name"),
];

pub fn magic_constant_completions() -> Vec<CompletionItem> {
    PHP_MAGIC_CONSTANTS
        .iter()
        .map(|(name, doc)| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::CONSTANT),
            detail: Some(doc.to_string()),
            ..Default::default()
        })
        .collect()
}

pub fn symbol_completions(doc: &ParsedDoc) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    collect_from_statements(&doc.program().stmts, &mut items);
    items
}

/// Like `symbol_completions` but only includes variables declared at or before `line`.
/// Non-variable items (functions, classes, etc.) are always included.
pub fn symbol_completions_before(doc: &ParsedDoc, line: u32) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    collect_from_statements_before(&doc.program().stmts, &mut items, line, doc.source());
    items
}

fn collect_from_statements_before(
    stmts: &[Stmt<'_, '_>],
    items: &mut Vec<CompletionItem>,
    line: u32,
    source: &str,
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
                    collect_from_statements_before(inner, items, line, source);
                }
            }
            // Non-variable items: always include
            _ => {
                collect_from_statements(std::slice::from_ref(stmt), items);
            }
        }
    }
}

fn collect_from_statements(stmts: &[Stmt<'_, '_>], items: &mut Vec<CompletionItem>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                items.push(callable_item(
                    f.name,
                    CompletionItemKind::FUNCTION,
                    !f.params.is_empty(),
                ));
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
                            items.push(callable_item(
                                m.name,
                                CompletionItemKind::METHOD,
                                !m.params.is_empty(),
                            ));
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

fn all_instance_members(
    class_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
) -> Vec<CompletionItem> {
    let all: Vec<&ParsedDoc> = std::iter::once(doc)
        .chain(other_docs.iter().map(|d| d.as_ref()))
        .collect();
    let mut items = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Queue: class names to process (inheritance chain + mixin chains).
    let mut queue: Vec<String> = vec![class_name.to_string()];
    while let Some(current) = queue.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let mut parent: Option<String> = None;
        for d in &all {
            let members = members_of_class(d, &current);
            if parent.is_none() {
                parent = members.parent.clone();
            }
            for (name, is_static) in members.methods {
                if !is_static && seen_names.insert(name.clone()) {
                    // Method params unknown here; use has_params=true so
                    // snippet cursor lands inside parens.
                    items.push(callable_item(&name, CompletionItemKind::METHOD, true));
                }
            }
            for (name, is_static) in &members.properties {
                if !is_static {
                    let label = format!("${name}");
                    if seen_names.insert(label.clone()) {
                        let is_readonly = members.readonly_properties.contains(name);
                        items.push(CompletionItem {
                            label,
                            kind: Some(CompletionItemKind::PROPERTY),
                            detail: if is_readonly {
                                Some("readonly".to_string())
                            } else {
                                None
                            },
                            ..Default::default()
                        });
                    }
                }
            }
            // Built-in enum properties: every enum case has `->name: string`
            // and backed enums also have `->value`.
            if is_enum(d, &current) {
                if seen_names.insert("name".to_string()) {
                    items.push(CompletionItem {
                        label: "name".to_string(),
                        kind: Some(CompletionItemKind::PROPERTY),
                        detail: Some("string".to_string()),
                        ..Default::default()
                    });
                }
                if is_backed_enum(d, &current) && seen_names.insert("value".to_string()) {
                    items.push(CompletionItem {
                        label: "value".to_string(),
                        kind: Some(CompletionItemKind::PROPERTY),
                        detail: Some("string|int".to_string()),
                        ..Default::default()
                    });
                }
            }
            // Collect @mixin classes for this class in this doc.
            for mixin in mixin_classes_of(d, &current) {
                queue.push(mixin);
            }
            // Queue trait names so their members are also included.
            for trait_name in members.trait_uses {
                queue.push(trait_name);
            }
        }
        // Fall back to built-in stubs if the class wasn't found in any user doc
        if let Some(stub) = builtin_class_members(&current) {
            if parent.is_none() {
                parent = stub.parent.clone();
            }
            for (name, is_static) in &stub.methods {
                if !is_static && seen_names.insert(name.clone()) {
                    items.push(callable_item(name, CompletionItemKind::METHOD, true));
                }
            }
            for (name, is_static) in &stub.properties {
                if !is_static {
                    let label = format!("${name}");
                    if seen_names.insert(label.clone()) {
                        items.push(CompletionItem {
                            label,
                            kind: Some(CompletionItemKind::PROPERTY),
                            ..Default::default()
                        });
                    }
                }
            }
            for name in &stub.constants {
                if seen_names.insert(name.clone()) {
                    items.push(CompletionItem {
                        label: name.clone(),
                        kind: Some(CompletionItemKind::CONSTANT),
                        ..Default::default()
                    });
                }
            }
        }
        if let Some(p) = parent {
            queue.push(p);
        }
    }
    items
}

fn all_static_members(
    class_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
) -> Vec<CompletionItem> {
    let all: Vec<&ParsedDoc> = std::iter::once(doc)
        .chain(other_docs.iter().map(|d| d.as_ref()))
        .collect();
    let mut items = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut queue: Vec<String> = vec![class_name.to_string()];
    while let Some(current) = queue.pop() {
        if !visited.insert(current.clone()) {
            continue;
        }
        let mut parent: Option<String> = None;
        for d in &all {
            let members = members_of_class(d, &current);
            if parent.is_none() {
                parent = members.parent.clone();
            }
            for (name, is_static) in members.methods {
                if is_static && seen_names.insert(name.clone()) {
                    items.push(callable_item(&name, CompletionItemKind::METHOD, true));
                }
            }
            for (name, is_static) in members.properties {
                if is_static {
                    let label = format!("${name}");
                    if seen_names.insert(label.clone()) {
                        items.push(CompletionItem {
                            label,
                            kind: Some(CompletionItemKind::PROPERTY),
                            ..Default::default()
                        });
                    }
                }
            }
            for name in members.constants {
                if seen_names.insert(name.clone()) {
                    items.push(CompletionItem {
                        label: name,
                        kind: Some(CompletionItemKind::CONSTANT),
                        ..Default::default()
                    });
                }
            }
            // Queue trait names so their static members are also included.
            for trait_name in members.trait_uses {
                queue.push(trait_name);
            }
        }
        // Fall back to built-in stubs for static members
        if let Some(stub) = builtin_class_members(&current) {
            if parent.is_none() {
                parent = stub.parent.clone();
            }
            for (name, is_static) in &stub.methods {
                if *is_static && seen_names.insert(name.clone()) {
                    items.push(callable_item(name, CompletionItemKind::METHOD, true));
                }
            }
            for (name, is_static) in &stub.properties {
                if *is_static {
                    let label = format!("${name}");
                    if seen_names.insert(label.clone()) {
                        items.push(CompletionItem {
                            label,
                            kind: Some(CompletionItemKind::PROPERTY),
                            ..Default::default()
                        });
                    }
                }
            }
            for name in &stub.constants {
                if seen_names.insert(name.clone()) {
                    items.push(CompletionItem {
                        label: name.clone(),
                        kind: Some(CompletionItemKind::CONSTANT),
                        ..Default::default()
                    });
                }
            }
        }
        if let Some(p) = parent {
            queue.push(p);
        }
    }
    items
}

/// Resolve `ClassName::` or the aliases `self::`, `static::`, `parent::`.
fn resolve_static_receiver(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    position: Position,
) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
    let before = before.strip_suffix("::").unwrap_or(before);
    let name: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '\\')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    match name.as_str() {
        "" => None,
        "self" | "static" => enclosing_class_at(source, doc, position),
        "parent" => {
            let enclosing = enclosing_class_at(source, doc, position)?;
            // Look for the parent class in current doc then other docs
            if let Some(p) = parent_class_name(doc, &enclosing) {
                return Some(p);
            }
            for other in other_docs {
                if let Some(p) = parent_class_name(other, &enclosing) {
                    return Some(p);
                }
            }
            None
        }
        _ => Some(name),
    }
}

/// If the `(` trigger occurs inside an attribute like `#[ClassName(`, extract
/// the attribute class name so we can offer its `__construct` parameter names.
fn resolve_attribute_class(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = line[..col].trim_end_matches('(').trim_end();
    // Look backwards on the same line for `#[ClassName` or `#[\NS\ClassName`
    let hash_pos = before.rfind("#[")?;
    let after_bracket = before[hash_pos + 2..].trim_start();
    // Strip leading backslashes (FQN), keep the short name
    let name: String = after_bracket
        .trim_start_matches('\\')
        .rsplit('\\')
        .next()
        .unwrap_or("")
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
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
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
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

/// Collect class/interface/trait/enum names with their FQN from an AST.
/// Handles both braced (`namespace Foo { ... }`) and unbraced (`namespace Foo;`) forms.
fn collect_classes_with_ns(
    stmts: &[Stmt<'_, '_>],
    ns_prefix: &str,
    items: &mut Vec<(String, CompletionItemKind, String)>,
) {
    // `cur_ns` tracks the namespace context for unbraced `namespace Foo;` declarations,
    // which apply to all subsequent statements at the same level.
    let mut cur_ns = ns_prefix.to_string();

    let fqn_for = |short: &str, ns: &str| -> String {
        if ns.is_empty() {
            short.to_string()
        } else {
            format!("{}\\{}", ns, short)
        }
    };

    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let short = c.name.unwrap_or("");
                if !short.is_empty() {
                    items.push((
                        short.to_string(),
                        CompletionItemKind::CLASS,
                        fqn_for(short, &cur_ns),
                    ));
                }
            }
            StmtKind::Interface(i) => {
                items.push((
                    i.name.to_string(),
                    CompletionItemKind::INTERFACE,
                    fqn_for(i.name, &cur_ns),
                ));
            }
            StmtKind::Trait(t) => {
                items.push((
                    t.name.to_string(),
                    CompletionItemKind::CLASS,
                    fqn_for(t.name, &cur_ns),
                ));
            }
            StmtKind::Enum(e) => {
                items.push((
                    e.name.to_string(),
                    CompletionItemKind::ENUM,
                    fqn_for(e.name, &cur_ns),
                ));
            }
            StmtKind::Namespace(ns) => {
                let ns_name = ns
                    .name
                    .as_ref()
                    .map(|n| n.to_string_repr().to_string())
                    .unwrap_or_default();
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        collect_classes_with_ns(inner, &ns_name, items);
                    }
                    NamespaceBody::Simple => {
                        // Unbraced namespace: applies to all subsequent statements.
                        cur_ns = ns_name;
                    }
                }
            }
            _ => {}
        }
    }
}

/// The line+col where a new `use` statement should be inserted in the current file.
fn use_insert_position(source: &str) -> Position {
    let mut last_use_line: Option<u32> = None;
    let mut anchor_line: u32 = 0;
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("<?") || trimmed.starts_with("namespace ") {
            anchor_line = i as u32;
        }
        if trimmed.starts_with("use ") && !trimmed.starts_with("use function ") {
            last_use_line = Some(i as u32);
        }
    }
    Position {
        line: last_use_line.unwrap_or(anchor_line) + 1,
        character: 0,
    }
}

/// The namespace declared at the top of the given statements, if any.
fn current_file_namespace(stmts: &[Stmt<'_, '_>]) -> String {
    for stmt in stmts {
        if let StmtKind::Namespace(ns) = &stmt.kind {
            return ns
                .name
                .as_ref()
                .map(|n| n.to_string_repr().to_string())
                .unwrap_or_default();
        }
    }
    String::new()
}

const PHP_MAGIC_METHODS: &[(&str, &str)] = &[
    (
        "__construct",
        "public function __construct($1)\n{\n    $2\n}",
    ),
    ("__destruct", "public function __destruct()\n{\n    $1\n}"),
    (
        "__get",
        "public function __get(string $name): mixed\n{\n    $1\n}",
    ),
    (
        "__set",
        "public function __set(string $name, mixed $value): void\n{\n    $1\n}",
    ),
    (
        "__isset",
        "public function __isset(string $name): bool\n{\n    $1\n}",
    ),
    (
        "__unset",
        "public function __unset(string $name): void\n{\n    $1\n}",
    ),
    (
        "__call",
        "public function __call(string $name, array $arguments): mixed\n{\n    $1\n}",
    ),
    (
        "__callStatic",
        "public static function __callStatic(string $name, array $arguments): mixed\n{\n    $1\n}",
    ),
    (
        "__toString",
        "public function __toString(): string\n{\n    $1\n}",
    ),
    (
        "__invoke",
        "public function __invoke($1): mixed\n{\n    $2\n}",
    ),
    ("__clone", "public function __clone(): void\n{\n    $1\n}"),
    ("__sleep", "public function __sleep(): array\n{\n    $1\n}"),
    ("__wakeup", "public function __wakeup(): void\n{\n    $1\n}"),
    (
        "__serialize",
        "public function __serialize(): array\n{\n    $1\n}",
    ),
    (
        "__unserialize",
        "public function __unserialize(array $data): void\n{\n    $1\n}",
    ),
    (
        "__debugInfo",
        "public function __debugInfo(): ?array\n{\n    $1\n}",
    ),
];

fn magic_method_completions() -> Vec<CompletionItem> {
    PHP_MAGIC_METHODS
        .iter()
        .map(|(name, snippet)| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::METHOD),
            insert_text: Some(snippet.to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            detail: Some("magic method".to_string()),
            ..Default::default()
        })
        .collect()
}

/// Completions filtered by trigger character, with optional `source` + `position`
/// so that `->` completions can be scoped to the variable's class.
pub fn filtered_completions_at(
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    trigger_character: Option<&str>,
    source: Option<&str>,
    position: Option<Position>,
    meta: Option<&PhpStormMeta>,
    doc_uri: Option<&Url>,
) -> Vec<CompletionItem> {
    match trigger_character {
        Some("$") => {
            let mut items = superglobal_completions();
            items.extend(
                symbol_completions(doc)
                    .into_iter()
                    .filter(|i| i.kind == Some(CompletionItemKind::VARIABLE)),
            );
            items
        }
        Some(">") => {
            // Arrow: $obj->  or  $this->
            if let (Some(src), Some(pos)) = (source, position) {
                let type_map = TypeMap::from_docs_with_meta(doc, other_docs, meta);
                if let Some(class_names) = resolve_receiver_class(src, doc, pos, &type_map) {
                    // Feature 5: support union types (Foo|Bar)
                    let mut items = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for class_name in class_names.split('|') {
                        let class_name = class_name.trim();
                        for item in all_instance_members(class_name, doc, other_docs) {
                            if seen.insert(item.label.clone()) {
                                items.push(item);
                            }
                        }
                    }
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
            // Static access: ClassName:: / self:: / static:: / parent::
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(class_name) = resolve_static_receiver(src, doc, other_docs, pos)
            {
                let items = all_static_members(&class_name, doc, other_docs);
                if !items.is_empty() {
                    return items;
                }
            }
            vec![]
        }
        Some("[") => {
            // PHP attribute: #[ — suggest attribute classes
            if let (Some(src), Some(pos)) = (source, position) {
                let line = src.lines().nth(pos.line as usize).unwrap_or("");
                let col = utf16_offset_to_byte(line, pos.character as usize);
                let before = &line[..col];
                if before.trim_end_matches('[').trim_end().ends_with('#') {
                    let mut items: Vec<CompletionItem> = Vec::new();
                    // Include classes from current doc and other docs
                    let mut classes = Vec::new();
                    collect_classes_with_ns(&doc.program().stmts, "", &mut classes);
                    for other in other_docs {
                        collect_classes_with_ns(&other.program().stmts, "", &mut classes);
                    }
                    let mut seen = std::collections::HashSet::new();
                    for (label, _kind, _fqn) in classes {
                        if seen.insert(label.clone()) {
                            items.push(CompletionItem {
                                label,
                                kind: Some(CompletionItemKind::CLASS),
                                ..Default::default()
                            });
                        }
                    }
                    return items;
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
                // Attribute constructor: #[ClassName(
                if let Some(attr_class) = resolve_attribute_class(src, pos) {
                    let mut attr_params = params_of_method(doc, &attr_class, "__construct");
                    if attr_params.is_empty() {
                        for other in other_docs {
                            attr_params = params_of_method(other, &attr_class, "__construct");
                            if !attr_params.is_empty() {
                                break;
                            }
                        }
                    }
                    if !attr_params.is_empty() {
                        return attr_params
                            .into_iter()
                            .map(|p| CompletionItem {
                                label: format!("{p}:"),
                                kind: Some(CompletionItemKind::VARIABLE),
                                detail: Some(format!("#{attr_class} argument")),
                                ..Default::default()
                            })
                            .collect();
                    }
                }
            }
            vec![]
        }
        _ => {
            // Feature 4: detect `use ` context and suggest FQNs from other docs
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(use_prefix) = use_completion_prefix(src, pos)
            {
                let mut use_items: Vec<CompletionItem> = Vec::new();
                for other in other_docs {
                    collect_fqns_with_prefix(
                        &other.program().stmts,
                        "",
                        &use_prefix,
                        &mut use_items,
                    );
                }
                // Also check current doc
                collect_fqns_with_prefix(&doc.program().stmts, "", &use_prefix, &mut use_items);
                if !use_items.is_empty() {
                    return use_items;
                }
            }

            // Feature 9: include/require path completions
            if let (Some(src), Some(pos), Some(uri)) = (source, position, doc_uri)
                && let Some(prefix) = include_path_prefix(src, pos)
            {
                let items = include_path_completions(uri, &prefix);
                if !items.is_empty() {
                    return items;
                }
            }

            // Feature 3: Sub-namespace \ completions outside use statement
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(prefix) = typed_prefix(Some(src), Some(pos))
                && prefix.contains('\\')
            {
                // Check we're NOT in a use statement
                let is_use = use_completion_prefix(src, pos).is_some();
                if !is_use {
                    let mut ns_items: Vec<CompletionItem> = Vec::new();
                    for other in other_docs {
                        let mut classes = Vec::new();
                        collect_classes_with_ns(&other.program().stmts, "", &mut classes);
                        for (label, kind, fqn) in classes {
                            if fqn.to_lowercase().starts_with(&prefix.to_lowercase()) {
                                ns_items.push(CompletionItem {
                                    label: label.clone(),
                                    kind: Some(kind),
                                    insert_text: Some(label),
                                    detail: Some(fqn),
                                    ..Default::default()
                                });
                            }
                        }
                    }
                    let mut classes = Vec::new();
                    collect_classes_with_ns(&doc.program().stmts, "", &mut classes);
                    for (label, kind, fqn) in classes {
                        if fqn.to_lowercase().starts_with(&prefix.to_lowercase()) {
                            ns_items.push(CompletionItem {
                                label: label.clone(),
                                kind: Some(kind),
                                insert_text: Some(label),
                                detail: Some(fqn),
                                ..Default::default()
                            });
                        }
                    }
                    if !ns_items.is_empty() {
                        return ns_items;
                    }
                }
            }

            // Feature 7: match arm completions
            if let (Some(src), Some(pos)) = (source, position)
                && let Some(match_items) = match_arm_completions(src, doc, other_docs, pos, meta)
                && !match_items.is_empty()
            {
                let mut all = match_items;
                // extend with normal items below, but return early here
                let mut normal_items = keyword_completions();
                normal_items.extend(magic_constant_completions());
                normal_items.extend(builtin_completions());
                normal_items.extend(superglobal_completions());
                normal_items.extend(symbol_completions(doc));
                all.extend(normal_items);
                return all;
            }

            // Feature 5: Magic method completions in class body
            let mut magic_items: Vec<CompletionItem> = Vec::new();
            if let (Some(src), Some(pos)) = (source, position)
                && enclosing_class_at(src, doc, pos).is_some()
            {
                magic_items.extend(magic_method_completions());
            }

            let mut items = keyword_completions();
            items.extend(magic_constant_completions());
            items.extend(builtin_completions());
            items.extend(superglobal_completions());
            // Feature 2: scope variable completions to before cursor line
            let sym_items = if let (Some(_src), Some(pos)) = (source, position) {
                symbol_completions_before(doc, pos.line)
            } else {
                symbol_completions(doc)
            };
            items.extend(sym_items);
            items.extend(magic_items);

            // Pre-compute use-import context for the current file.
            let use_map = source.map(|_| UseMap::from_doc(doc));
            let cur_ns = current_file_namespace(&doc.program().stmts);

            for other in other_docs {
                // Class-like symbols: add `use` insertion when needed.
                let mut classes: Vec<(String, CompletionItemKind, String)> = Vec::new();
                collect_classes_with_ns(&other.program().stmts, "", &mut classes);
                for (label, kind, fqn) in classes {
                    let additional_text_edits =
                        if let (Some(src), Some(umap)) = (source, use_map.as_ref()) {
                            let in_same_ns =
                                !cur_ns.is_empty() && fqn == format!("{}\\{}", cur_ns, label);
                            let is_global = !fqn.contains('\\');
                            let already = umap.resolve(&label).is_some();
                            if !in_same_ns && !is_global && !already {
                                let pos = use_insert_position(src);
                                Some(vec![TextEdit {
                                    range: Range {
                                        start: pos,
                                        end: pos,
                                    },
                                    new_text: format!("use {};\n", fqn),
                                }])
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                    items.push(CompletionItem {
                        label,
                        kind: Some(kind),
                        detail: if fqn.contains('\\') { Some(fqn) } else { None },
                        additional_text_edits,
                        ..Default::default()
                    });
                }
                // Non-class symbols (functions, methods, constants) need no use statement.
                let cross: Vec<CompletionItem> = symbol_completions(other)
                    .into_iter()
                    .filter(|i| {
                        !matches!(
                            i.kind,
                            Some(CompletionItemKind::CLASS)
                                | Some(CompletionItemKind::INTERFACE)
                                | Some(CompletionItemKind::ENUM)
                        ) && i.kind != Some(CompletionItemKind::VARIABLE)
                    })
                    .collect();
                items.extend(cross);
            }
            let mut seen = std::collections::HashSet::new();
            items.retain(|i| seen.insert(i.label.clone()));

            // Extract the typed prefix for fuzzy camel/underscore filtering.
            let prefix = typed_prefix(source, position).unwrap_or_default();
            if prefix.contains('\\') {
                // Namespace-qualified prefix: filter by FQN prefix match.
                let ns_prefix = prefix.trim_start_matches('\\').to_lowercase();
                items.retain(|i| {
                    let fqn = i.detail.as_deref().unwrap_or(&i.label);
                    fqn.to_lowercase().starts_with(&ns_prefix)
                });
            } else if !prefix.is_empty() {
                items.retain(|i| fuzzy_camel_match(&prefix, &i.label));
                for item in &mut items {
                    item.sort_text = Some(camel_sort_key(&prefix, &item.label));
                    item.filter_text = Some(item.label.clone());
                }
            }
            items
        }
    }
}

fn match_arm_completions(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    position: Position,
    meta: Option<&PhpStormMeta>,
) -> Option<Vec<CompletionItem>> {
    let start_line = position.line as usize;
    let end_line = start_line.saturating_sub(5);
    for line_idx in (end_line..=start_line).rev() {
        let line = source.lines().nth(line_idx)?;
        if let Some(cap) = extract_match_subject(line) {
            let type_map = TypeMap::from_docs_with_meta(doc, other_docs, meta);
            let class_name = if cap == "this" {
                enclosing_class_at(source, doc, position)?
            } else {
                type_map.get(&format!("${cap}"))?.to_string()
            };
            let all_docs: Vec<&ParsedDoc> = std::iter::once(doc)
                .chain(other_docs.iter().map(|d| d.as_ref()))
                .collect();
            for d in &all_docs {
                let members = members_of_class(d, &class_name);
                if !members.constants.is_empty() {
                    return Some(
                        members
                            .constants
                            .iter()
                            .map(|c| CompletionItem {
                                label: format!("{class_name}::{c}"),
                                kind: Some(CompletionItemKind::CONSTANT),
                                ..Default::default()
                            })
                            .collect(),
                    );
                }
            }
        }
    }
    None
}

/// Returns the path prefix typed inside a string on an include/require line, or None.
fn include_path_prefix(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let trimmed = line.trim_start();
    if !trimmed.starts_with("include") && !trimmed.starts_with("require") {
        return None;
    }
    // Find the string being typed
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
    let quote_pos = before.rfind(['\'', '"'])?;
    Some(before[quote_pos + 1..].to_string())
}

/// Build completion items for include/require path strings.
/// `prefix` is the partial path typed so far (e.g. `"../lib/"` or `"./"`).
fn include_path_completions(doc_uri: &Url, prefix: &str) -> Vec<CompletionItem> {
    use std::path::Path;

    let doc_path = match doc_uri.to_file_path() {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    let doc_dir = match doc_path.parent() {
        Some(d) => d.to_path_buf(),
        None => return vec![],
    };

    // Resolve the directory to list: join the doc dir with the prefix's directory component.
    let (dir_to_list, typed_file) = if prefix.ends_with('/') || prefix.ends_with('\\') {
        (doc_dir.join(prefix), String::new())
    } else {
        let p = Path::new(prefix);
        let parent = p
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file = p
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        (doc_dir.join(&parent), file)
    };

    let entries = match std::fs::read_dir(&dir_to_list) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut items = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Skip hidden files unless the prefix already starts with a dot
        if name.starts_with('.') && !typed_file.starts_with('.') {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let is_php = name.ends_with(".php") || name.ends_with(".inc") || name.ends_with(".phtml");
        if !is_dir && !is_php {
            continue;
        }
        let insert = if is_dir {
            format!("{}/", name)
        } else {
            name.clone()
        };
        items.push(CompletionItem {
            label: name,
            kind: Some(if is_dir {
                CompletionItemKind::FOLDER
            } else {
                CompletionItemKind::FILE
            }),
            insert_text: Some(insert),
            ..Default::default()
        });
    }
    items.sort_by(|a, b| {
        // Directories first, then files
        let a_dir = a.kind == Some(CompletionItemKind::FOLDER);
        let b_dir = b.kind == Some(CompletionItemKind::FOLDER);
        b_dir.cmp(&a_dir).then(a.label.cmp(&b.label))
    });
    items
}

fn extract_match_subject(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let after = trimmed.strip_prefix("match")?.trim_start();
    let after = after.strip_prefix('(')?;
    let inner: String = after.chars().take_while(|&c| c != ')').collect();
    let var = inner.trim().trim_start_matches('$');
    if var.is_empty() {
        None
    } else {
        Some(var.to_string())
    }
}

/// Returns the prefix typed after `use ` on the current line, or None if not in a use statement.
fn use_completion_prefix(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = line[..col].trim_start();
    let prefix = before.strip_prefix("use ")?;
    Some(prefix.trim_start_matches('\\').to_string())
}

/// Collect fully-qualified names from stmts that contain `prefix`.
fn collect_fqns_with_prefix(
    stmts: &[Stmt<'_, '_>],
    ns: &str,
    prefix: &str,
    out: &mut Vec<CompletionItem>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                if let Some(name) = c.name {
                    let fqn = if ns.is_empty() {
                        name.to_string()
                    } else {
                        format!("{ns}\\{name}")
                    };
                    if fqn.to_lowercase().contains(&prefix.to_lowercase()) || prefix.is_empty() {
                        out.push(CompletionItem {
                            label: fqn.clone(),
                            kind: Some(CompletionItemKind::CLASS),
                            insert_text: Some(fqn),
                            ..Default::default()
                        });
                    }
                }
            }
            StmtKind::Interface(i) => {
                let fqn = if ns.is_empty() {
                    i.name.to_string()
                } else {
                    format!("{ns}\\{}", i.name)
                };
                if fqn.to_lowercase().contains(&prefix.to_lowercase()) || prefix.is_empty() {
                    out.push(CompletionItem {
                        label: fqn.clone(),
                        kind: Some(CompletionItemKind::INTERFACE),
                        insert_text: Some(fqn),
                        ..Default::default()
                    });
                }
            }
            StmtKind::Namespace(ns_stmt) => {
                let ns_name = ns_stmt
                    .name
                    .as_ref()
                    .map(|n| {
                        if ns.is_empty() {
                            n.to_string_repr().to_string()
                        } else {
                            format!("{ns}\\{}", n.to_string_repr())
                        }
                    })
                    .unwrap_or_else(|| ns.to_string());
                if let NamespaceBody::Braced(inner) = &ns_stmt.body {
                    collect_fqns_with_prefix(inner, &ns_name, prefix, out);
                }
            }
            _ => {}
        }
    }
}

/// Extract the identifier characters typed immediately before the cursor.
/// Includes `\` to support namespace-qualified prefixes like `App\Serv`.
fn typed_prefix(source: Option<&str>, position: Option<Position>) -> Option<String> {
    let src = source?;
    let pos = position?;
    let line = src.lines().nth(pos.line as usize)?;
    let col = utf16_offset_to_byte(line, pos.character as usize);
    let before = &line[..col];
    let prefix: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '\\')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if prefix.is_empty() {
        None
    } else {
        Some(prefix)
    }
}

fn resolve_receiver_class(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    type_map: &TypeMap,
) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..col];
    // Try ?-> first (longer pattern) so `$s?->` doesn't get stripped to `$s?` by the `->` rule.
    let before = before
        .strip_suffix("?->")
        .or_else(|| before.strip_suffix("->"))
        .unwrap_or(before);

    // Handle (new ClassName()) before ->
    if let Some(class_name) = extract_new_class_before_arrow(before) {
        return Some(class_name);
    }

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
        // Prefer the enclosing class (standard method context).
        // Fall back to type_map for top-level bound closures where
        // Closure::bind / bindTo / call injected a $this mapping.
        return enclosing_class_at(source, doc, position)
            .or_else(|| type_map.get("$this").map(|s| s.to_string()));
    }
    type_map.get(&var_name).map(|s| s.to_string())
}

/// Extract the class name from `(new ClassName(...))` or `new ClassName(...)` text
/// appearing immediately before `->`.
fn extract_new_class_before_arrow(text: &str) -> Option<String> {
    let text = text.trim_end();
    // Strip optional closing paren wrapping: `(new Foo())`
    let inner = if let Some(without_last) = text.strip_suffix(')') {
        // Find matching open paren — look for `(new` pattern
        if let Some(pos) = without_last.rfind("(new ") {
            &without_last[pos + 1..]
        } else if let Some(pos) = without_last.rfind("(new\t") {
            &without_last[pos + 1..]
        } else {
            text
        }
    } else {
        text
    };
    // Now inner should start with `new ClassName(...)`
    let inner = inner.trim();
    if !inner.starts_with("new ") && !inner.starts_with("new\t") {
        return None;
    }
    let after_new = inner[3..].trim_start();
    // Extract class name (alphanumeric + _ + \)
    let class: String = after_new
        .chars()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '\\')
        .collect();
    if class.is_empty() {
        return None;
    }
    // Return short name
    Some(class.rsplit('\\').next().unwrap_or(&class).to_string())
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
        let kws = keyword_completions();
        assert!(
            kws.len() >= 20,
            "expected at least 20 keywords, got {}",
            kws.len()
        );
    }

    #[test]
    fn keywords_contain_common_php_keywords() {
        let kws = keyword_completions();
        let ls = labels(&kws);
        for expected in &[
            "function",
            "class",
            "return",
            "foreach",
            "match",
            "namespace",
        ] {
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
    fn magic_constants_all_present() {
        let items = magic_constant_completions();
        let ls: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        for name in &[
            "__FILE__",
            "__DIR__",
            "__LINE__",
            "__CLASS__",
            "__FUNCTION__",
            "__METHOD__",
            "__NAMESPACE__",
            "__TRAIT__",
        ] {
            assert!(ls.contains(name), "missing magic constant: {name}");
        }
    }

    #[test]
    fn magic_constants_have_constant_kind() {
        for item in magic_constant_completions() {
            assert_eq!(
                item.kind,
                Some(CompletionItemKind::CONSTANT),
                "{} should have CONSTANT kind",
                item.label
            );
        }
    }

    #[test]
    fn resolve_attribute_class_extracts_name() {
        let src = "<?php\n#[Route(\n";
        // Position right after the '(' on line 1
        let pos = Position {
            line: 1,
            character: 8,
        };
        let result = resolve_attribute_class(src, pos);
        assert_eq!(result.as_deref(), Some("Route"));
    }

    #[test]
    fn resolve_attribute_class_fqn_extracts_short_name() {
        let src = "<?php\n#[\\Symfony\\Component\\Routing\\Route(\n";
        let pos = Position {
            line: 1,
            character: 38,
        };
        let result = resolve_attribute_class(src, pos);
        assert_eq!(result.as_deref(), Some("Route"));
    }

    #[test]
    fn resolve_attribute_class_returns_none_for_regular_call() {
        let src = "<?php\nsomeFunction(\n";
        let pos = Position {
            line: 1,
            character: 14,
        };
        let result = resolve_attribute_class(src, pos);
        assert!(result.is_none(), "should not match regular function call");
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
        for item in items
            .iter()
            .filter(|i| i.label == "add" || i.label == "sub")
        {
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
        for item in items
            .iter()
            .filter(|i| i.label == "$name" || i.label == "$age")
        {
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
        let items = filtered_completions_at(&d, &[], Some("$"), None, None, None, None);
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
        let items = filtered_completions_at(&d, &[], Some(">"), None, None, None, None);
        assert!(!items.is_empty(), "should have method items");
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        }
    }

    #[test]
    fn none_trigger_returns_keywords_functions_classes() {
        let d = doc("<?php\nfunction greet() {}\nclass MyApp {}");
        let items = filtered_completions_at(&d, &[], None, None, None, None, None);
        let ls = labels(&items);
        assert!(
            ls.contains(&"function"),
            "should contain keyword 'function'"
        );
        assert!(ls.contains(&"greet"), "should contain function 'greet'");
        assert!(ls.contains(&"MyApp"), "should contain class 'MyApp'");
    }

    #[test]
    fn builtins_appear_in_default_completions() {
        let d = doc("<?php");
        let items = filtered_completions_at(&d, &[], None, None, None, None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"strlen"), "missing strlen");
        assert!(ls.contains(&"array_map"), "missing array_map");
        assert!(ls.contains(&"json_encode"), "missing json_encode");
    }

    #[test]
    fn colon_trigger_returns_static_members() {
        let src = "<?php\nclass Cfg { public static function load(): void {} public static int $debug = 0; const VERSION = '1'; }\nCfg::";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 5,
        };
        let items = filtered_completions_at(&d, &[], Some(":"), Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"load"), "missing static method");
        assert!(ls.contains(&"VERSION"), "missing constant");
    }

    #[test]
    fn inherited_methods_appear_in_arrow_completion() {
        let src = "<?php\nclass Base { public function baseMethod() {} }\nclass Child extends Base { public function childMethod() {} }\n$c = new Child();\n$c->";
        let d = doc(src);
        let pos = Position {
            line: 4,
            character: 4,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"baseMethod"), "missing inherited baseMethod");
        assert!(ls.contains(&"childMethod"), "missing childMethod");
    }

    #[test]
    fn param_named_arg_completion() {
        let src = "<?php\nfunction connect(string $host, int $port): void {}\nconnect(";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 8,
        };
        let items = filtered_completions_at(&d, &[], Some("("), Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"host:"), "missing host:");
        assert!(ls.contains(&"port:"), "missing port:");
    }

    #[test]
    fn cross_file_symbols_appear_in_default_completions() {
        let d = doc("<?php\nfunction localFn() {}");
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nclass RemoteService {}\nfunction remoteHelper() {}".to_string(),
        ));
        let items = filtered_completions_at(&d, &[other], None, None, None, None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"localFn"), "missing local function");
        assert!(ls.contains(&"RemoteService"), "missing cross-file class");
        assert!(ls.contains(&"remoteHelper"), "missing cross-file function");
    }

    #[test]
    fn cross_file_variables_not_included_in_default_completions() {
        let d = doc("<?php\n$localVar = 1;");
        let other = Arc::new(ParsedDoc::parse("<?php\n$remoteVar = 2;".to_string()));
        let items = filtered_completions_at(&d, &[other], None, None, None, None, None);
        let ls = labels(&items);
        assert!(
            !ls.contains(&"$remoteVar"),
            "cross-file variable should not appear"
        );
    }

    #[test]
    fn cross_file_class_gets_use_insertion() {
        let current_src = "<?php\nnamespace App;\n\n$x = new ";
        let d = doc(current_src);
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace Lib;\nclass Mailer {}".to_string(),
        ));
        let pos = Position {
            line: 3,
            character: 9,
        };
        let items =
            filtered_completions_at(&d, &[other], None, Some(current_src), Some(pos), None, None);
        let mailer = items.iter().find(|i| i.label == "Mailer");
        assert!(mailer.is_some(), "Mailer should appear in completions");
        let edits = mailer.unwrap().additional_text_edits.as_ref();
        assert!(edits.is_some(), "Mailer should have additionalTextEdits");
        let edit_text = &edits.unwrap()[0].new_text;
        assert!(
            edit_text.contains("use Lib\\Mailer;"),
            "edit should insert 'use Lib\\Mailer;', got: {edit_text}"
        );
    }

    #[test]
    fn same_namespace_class_gets_no_use_insertion() {
        let current_src = "<?php\nnamespace Lib;\n$x = new ";
        let d = doc(current_src);
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace Lib;\nclass Mailer {}".to_string(),
        ));
        let pos = Position {
            line: 2,
            character: 9,
        };
        let items =
            filtered_completions_at(&d, &[other], None, Some(current_src), Some(pos), None, None);
        let mailer = items.iter().find(|i| i.label == "Mailer");
        assert!(mailer.is_some(), "Mailer should appear in completions");
        assert!(
            mailer.unwrap().additional_text_edits.is_none(),
            "same-namespace class should not get a use edit"
        );
    }

    #[test]
    fn function_with_params_gets_snippet() {
        let d = doc("<?php\nfunction process($input) {}");
        let items = symbol_completions(&d);
        let item = items.iter().find(|i| i.label == "process").unwrap();
        assert_eq!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_eq!(item.insert_text.as_deref(), Some("process($1)"));
    }

    #[test]
    fn function_without_params_gets_plain_call() {
        let d = doc("<?php\nfunction doThing() {}");
        let items = symbol_completions(&d);
        let item = items.iter().find(|i| i.label == "doThing").unwrap();
        // No snippet format needed for zero-arg functions.
        assert_eq!(item.insert_text.as_deref(), Some("doThing()"));
        assert_ne!(item.insert_text_format, Some(InsertTextFormat::SNIPPET));
    }

    #[test]
    fn builtin_functions_get_snippet() {
        let items = builtin_completions();
        let strlen = items.iter().find(|i| i.label == "strlen").unwrap();
        assert_eq!(strlen.insert_text_format, Some(InsertTextFormat::SNIPPET));
        assert_eq!(strlen.insert_text.as_deref(), Some("strlen($1)"));
    }

    #[test]
    fn enum_arrow_completion_includes_name_property() {
        let src = "<?php\nenum Suit { case Hearts; }\n$s = new Suit();\n$s->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        assert!(
            items.iter().any(|i| i.label == "name"),
            "enum should have ->name"
        );
    }

    #[test]
    fn backed_enum_arrow_completion_includes_value_property() {
        let src =
            "<?php\nenum Status: string { case Active = 'active'; }\n$s = new Status();\n$s->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        assert!(
            items.iter().any(|i| i.label == "name"),
            "backed enum should have ->name"
        );
        assert!(
            items.iter().any(|i| i.label == "value"),
            "backed enum should have ->value"
        );
    }

    #[test]
    fn pure_enum_arrow_completion_has_no_value_property() {
        let src = "<?php\nenum Suit { case Hearts; }\n$s = new Suit();\n$s->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        assert!(
            !items.iter().any(|i| i.label == "value"),
            "pure enum should not have ->value"
        );
    }

    #[test]
    fn superglobals_appear_on_dollar_trigger() {
        let d = doc("<?php\n");
        let items = filtered_completions_at(&d, &[], Some("$"), None, None, None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"$_SERVER"), "missing $_SERVER");
        assert!(ls.contains(&"$_GET"), "missing $_GET");
        assert!(ls.contains(&"$_POST"), "missing $_POST");
        assert!(ls.contains(&"$_SESSION"), "missing $_SESSION");
        assert!(ls.contains(&"$GLOBALS"), "missing $GLOBALS");
    }

    #[test]
    fn superglobals_appear_in_default_completions() {
        let d = doc("<?php\n");
        let items = filtered_completions_at(&d, &[], None, None, None, None, None);
        let ls = labels(&items);
        assert!(
            ls.contains(&"$_SERVER"),
            "missing $_SERVER in default completions"
        );
    }

    #[test]
    fn instanceof_narrowing_provides_arrow_completions() {
        // $x instanceof Foo should narrow $x to Foo inside the if body
        let src =
            "<?php\nclass Foo { public function doFoo() {} }\nif ($x instanceof Foo) {\n    $x->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 8,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(
            ls.contains(&"doFoo"),
            "instanceof narrowing should make Foo methods available"
        );
    }

    #[test]
    fn constructor_chain_arrow_completion() {
        let src = "<?php\nclass Builder { public function build() {} public function reset() {} }\n(new Builder())->";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 16,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(
            ls.contains(&"build"),
            "constructor chain should complete Builder methods"
        );
        assert!(
            ls.contains(&"reset"),
            "constructor chain should complete Builder methods"
        );
    }

    // Feature 4: use statement FQN completions
    #[test]
    fn use_statement_suggests_fqns() {
        let d = doc("<?php\nuse ");
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace App\\Services;\nclass Mailer {}".to_string(),
        ));
        let pos = Position {
            line: 1,
            character: 4,
        };
        let items = filtered_completions_at(
            &d,
            &[other],
            None,
            Some("<?php\nuse "),
            Some(pos),
            None,
            None,
        );
        assert!(
            items.iter().any(|i| i.label.contains("Mailer")),
            "use completion should suggest Mailer"
        );
    }

    // Feature 5: union type param completions
    #[test]
    fn union_type_param_completes_both_classes() {
        let src = "<?php\nclass Foo { public function fooMethod() {} }\nclass Bar { public function barMethod() {} }\n/**\n * @param Foo|Bar $x\n */\nfunction handle($x) {\n    $x->";
        let d = doc(src);
        let pos = Position {
            line: 7,
            character: 8,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(
            ls.contains(&"fooMethod"),
            "should complete Foo methods from union"
        );
        assert!(
            ls.contains(&"barMethod"),
            "should complete Bar methods from union"
        );
    }

    // Feature 6: attribute bracket completions
    #[test]
    fn attribute_bracket_suggests_classes() {
        let d = doc("<?php\nclass Route {}\nclass Middleware {}\n#[");
        let pos = Position {
            line: 3,
            character: 2,
        };
        let items = filtered_completions_at(
            &d,
            &[],
            Some("["),
            Some("<?php\nclass Route {}\nclass Middleware {}\n#["),
            Some(pos),
            None,
            None,
        );
        let ls = labels(&items);
        assert!(ls.contains(&"Route"), "should suggest Route as attribute");
        assert!(
            ls.contains(&"Middleware"),
            "should suggest Middleware as attribute"
        );
    }

    // Feature 7: match arm completions
    #[test]
    fn match_arm_suggests_enum_cases() {
        let src = "<?php\nenum Status { case Active; case Inactive; case Pending; }\n$s = new Status();\nmatch ($s) {\n    ";
        let d = doc(src);
        let pos = Position {
            line: 4,
            character: 4,
        };
        let items = filtered_completions_at(&d, &[], None, Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(
            ls.iter().any(|l| l.contains("Active")),
            "match should suggest Status::Active"
        );
    }

    // Feature 10: readonly property recognition
    #[test]
    fn readonly_property_has_detail_tag() {
        let src = "<?php\nclass Config { public readonly string $name; }\n$c = new Config();\n$c->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 4,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None); // trigger ">"
        let name_item = items.iter().find(|i| i.label == "$name");
        assert!(name_item.is_some(), "should have $name in completions");
        assert_eq!(
            name_item.unwrap().detail.as_deref(),
            Some("readonly"),
            "$name should be tagged readonly"
        );
    }

    // Feature 2: variables scoped to cursor line
    #[test]
    fn variables_after_cursor_not_suggested() {
        let src = "<?php\n$early = new Foo();\n// cursor here\n$late = new Bar();";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 0,
        };
        let items = filtered_completions_at(&d, &[], None, Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"$early"), "$early should be suggested");
        assert!(
            !ls.contains(&"$late"),
            "$late declared after cursor should not be suggested"
        );
    }

    // Feature 3: sub-namespace backslash completions
    #[test]
    fn backslash_prefix_suggests_matching_classes() {
        let d = doc("<?php\n$x = new App\\");
        let other = Arc::new(ParsedDoc::parse(
            "<?php\nnamespace App\\Services;\nclass Mailer {}\nclass Logger {}".to_string(),
        ));
        let pos = Position {
            line: 1,
            character: 18,
        };
        let items = filtered_completions_at(
            &d,
            &[other],
            None,
            Some("<?php\n$x = new App\\"),
            Some(pos),
            None,
            None,
        );
        let ls = labels(&items);
        assert!(
            ls.contains(&"Mailer"),
            "should suggest Mailer under App\\Services"
        );
    }

    // Feature 1: nullsafe ?-> completions
    #[test]
    fn nullsafe_arrow_triggers_member_completions() {
        let src = "<?php\nclass Service { public function run() {} public string $status; }\n$s = new Service();\n$s?->";
        let d = doc(src);
        let pos = Position {
            line: 3,
            character: 5,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"run"), "?-> should complete Service::run()");
        assert!(
            ls.iter().any(|l| l.contains("status")),
            "?-> should complete Service::$status"
        );
    }

    // Feature 5: magic methods in class body
    #[test]
    fn magic_methods_suggested_in_class_body() {
        let src = "<?php\nclass Foo {\n    __\n}";
        let d = doc(src);
        let pos = Position {
            line: 2,
            character: 6,
        };
        let items = filtered_completions_at(&d, &[], None, Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"__construct"), "should suggest __construct");
        assert!(ls.contains(&"__toString"), "should suggest __toString");
    }

    #[test]
    fn arrow_trigger_does_not_complete_on_unknown_receiver() {
        // $unknown-> has no type info, so no class members should be returned.
        // The fallback returns methods from the current doc, but since the doc
        // has no class, the result should be empty (no methods available).
        let src = "<?php\n$unknown->";
        let d = doc(src);
        let pos = Position {
            line: 1,
            character: 10,
        };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None, None);
        // No class is defined in this doc, so the fallback method list is empty.
        assert!(
            items.is_empty(),
            "unknown receiver should yield no completions, got: {:?}",
            labels(&items)
        );
    }

    #[test]
    fn static_trigger_shows_only_static_members() {
        // ClassName:: should only return static methods/constants, NOT instance methods.
        let src = concat!(
            "<?php\n",
            "class MyClass {\n",
            "    public static function staticMethod(): void {}\n",
            "    public function instanceMethod(): void {}\n",
            "    public static int $staticProp = 0;\n",
            "    const MY_CONST = 42;\n",
            "}\n",
            "MyClass::",
        );
        let d = doc(src);
        let pos = Position {
            line: 7,
            character: 9,
        };
        let items = filtered_completions_at(&d, &[], Some(":"), Some(src), Some(pos), None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"staticMethod"), "should include static method");
        assert!(ls.contains(&"MY_CONST"), "should include constant");
        assert!(
            !ls.contains(&"instanceMethod"),
            "should NOT include instance method in static completion, got: {:?}",
            ls
        );
    }
}
