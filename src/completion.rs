use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, InsertTextFormat, Position, Range, TextEdit,
};

use crate::ast::ParsedDoc;
use crate::phpstorm_meta::PhpStormMeta;
use crate::stubs::builtin_class_members;
use crate::use_resolver::UseMap;
use crate::type_map::{
    TypeMap, enclosing_class_at, is_backed_enum, is_enum, members_of_class, mixin_classes_of,
    params_of_function, parent_class_name,
};
use crate::util::{camel_sort_key, fuzzy_camel_match};

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

pub fn symbol_completions(doc: &ParsedDoc) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    collect_from_statements(&doc.program().stmts, &mut items);
    items
}

fn collect_from_statements(stmts: &[Stmt<'_, '_>], items: &mut Vec<CompletionItem>) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                items.push(callable_item(f.name, CompletionItemKind::FUNCTION, !f.params.is_empty()));
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
                            items.push(callable_item(m.name, CompletionItemKind::METHOD, !m.params.is_empty()));
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
            for (name, is_static) in members.properties {
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
                    items.push((short.to_string(), CompletionItemKind::CLASS, fqn_for(short, &cur_ns)));
                }
            }
            StmtKind::Interface(i) => {
                items.push((i.name.to_string(), CompletionItemKind::INTERFACE, fqn_for(i.name, &cur_ns)));
            }
            StmtKind::Trait(t) => {
                items.push((t.name.to_string(), CompletionItemKind::CLASS, fqn_for(t.name, &cur_ns)));
            }
            StmtKind::Enum(e) => {
                items.push((e.name.to_string(), CompletionItemKind::ENUM, fqn_for(e.name, &cur_ns)));
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

/// Completions filtered by trigger character, with optional `source` + `position`
/// so that `->` completions can be scoped to the variable's class.
pub fn filtered_completions_at(
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    trigger_character: Option<&str>,
    source: Option<&str>,
    position: Option<Position>,
    meta: Option<&PhpStormMeta>,
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
                if let Some(class_name) = resolve_receiver_class(src, doc, pos, &type_map) {
                    let items = all_instance_members(&class_name, doc, other_docs);
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
            if let (Some(src), Some(pos)) = (source, position) {
                if let Some(class_name) = resolve_static_receiver(src, doc, other_docs, pos) {
                    let items = all_static_members(&class_name, doc, other_docs);
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
            items.extend(superglobal_completions());
            items.extend(symbol_completions(doc));

            // Pre-compute use-import context for the current file.
            let use_map = source.map(|_| UseMap::from_doc(doc));
            let cur_ns = current_file_namespace(&doc.program().stmts);

            for other in other_docs {
                // Class-like symbols: add `use` insertion when needed.
                let mut classes: Vec<(String, CompletionItemKind, String)> = Vec::new();
                collect_classes_with_ns(&other.program().stmts, "", &mut classes);
                for (label, kind, fqn) in classes {
                    let additional_text_edits = if let (Some(src), Some(ref umap)) =
                        (source, use_map.as_ref())
                    {
                        let in_same_ns = !cur_ns.is_empty()
                            && fqn == format!("{}\\{}", cur_ns, label);
                        let is_global = !fqn.contains('\\');
                        let already = umap.resolve(&label).is_some();
                        if !in_same_ns && !is_global && !already {
                            let pos = use_insert_position(src);
                            Some(vec![TextEdit {
                                range: Range { start: pos, end: pos },
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

/// Extract the identifier characters typed immediately before the cursor.
/// Includes `\` to support namespace-qualified prefixes like `App\Serv`.
fn typed_prefix(source: Option<&str>, position: Option<Position>) -> Option<String> {
    let src = source?;
    let pos = position?;
    let line = src.lines().nth(pos.line as usize)?;
    let col = (pos.character as usize).min(line.len());
    let before = &line[..col];
    let prefix: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '\\')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if prefix.is_empty() { None } else { Some(prefix) }
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
    let before = before.strip_suffix("->").or_else(|| before.strip_suffix("?->")).unwrap_or(before);

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
    let inner = if text.ends_with(')') {
        let without_last = &text[..text.len()-1];
        // Find matching open paren — look for `(new` pattern
        if let Some(pos) = without_last.rfind("(new ") {
            &without_last[pos+1..]
        } else if let Some(pos) = without_last.rfind("(new\t") {
            &without_last[pos+1..]
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
        assert!(!keyword_completions().is_empty());
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
        let items = filtered_completions_at(&d, &[], Some("$"), None, None, None);
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
        let items = filtered_completions_at(&d, &[], Some(">"), None, None, None);
        assert!(!items.is_empty(), "should have method items");
        for item in &items {
            assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        }
    }

    #[test]
    fn none_trigger_returns_keywords_functions_classes() {
        let d = doc("<?php\nfunction greet() {}\nclass MyApp {}");
        let items = filtered_completions_at(&d, &[], None, None, None, None);
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
        let items = filtered_completions_at(&d, &[], None, None, None, None);
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
        let items = filtered_completions_at(&d, &[], Some(":"), Some(src), Some(pos), None);
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
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None);
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
        let items = filtered_completions_at(&d, &[], Some("("), Some(src), Some(pos), None);
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
        let items = filtered_completions_at(&d, &[other], None, None, None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"localFn"), "missing local function");
        assert!(ls.contains(&"RemoteService"), "missing cross-file class");
        assert!(ls.contains(&"remoteHelper"), "missing cross-file function");
    }

    #[test]
    fn cross_file_variables_not_included_in_default_completions() {
        let d = doc("<?php\n$localVar = 1;");
        let other = Arc::new(ParsedDoc::parse("<?php\n$remoteVar = 2;".to_string()));
        let items = filtered_completions_at(&d, &[other], None, None, None, None);
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
        let pos = Position { line: 3, character: 9 };
        let items = filtered_completions_at(&d, &[other], None, Some(current_src), Some(pos), None);
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
        let pos = Position { line: 2, character: 9 };
        let items = filtered_completions_at(&d, &[other], None, Some(current_src), Some(pos), None);
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
        let pos = Position { line: 3, character: 4 };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None);
        assert!(items.iter().any(|i| i.label == "name"), "enum should have ->name");
    }

    #[test]
    fn backed_enum_arrow_completion_includes_value_property() {
        let src = "<?php\nenum Status: string { case Active = 'active'; }\n$s = new Status();\n$s->";
        let d = doc(src);
        let pos = Position { line: 3, character: 4 };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None);
        assert!(items.iter().any(|i| i.label == "name"), "backed enum should have ->name");
        assert!(items.iter().any(|i| i.label == "value"), "backed enum should have ->value");
    }

    #[test]
    fn pure_enum_arrow_completion_has_no_value_property() {
        let src = "<?php\nenum Suit { case Hearts; }\n$s = new Suit();\n$s->";
        let d = doc(src);
        let pos = Position { line: 3, character: 4 };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None);
        assert!(!items.iter().any(|i| i.label == "value"), "pure enum should not have ->value");
    }

    #[test]
    fn superglobals_appear_on_dollar_trigger() {
        let d = doc("<?php\n");
        let items = filtered_completions_at(&d, &[], Some("$"), None, None, None);
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
        let items = filtered_completions_at(&d, &[], None, None, None, None);
        let ls = labels(&items);
        assert!(ls.contains(&"$_SERVER"), "missing $_SERVER in default completions");
    }

    #[test]
    fn instanceof_narrowing_provides_arrow_completions() {
        // $x instanceof Foo should narrow $x to Foo inside the if body
        let src = "<?php\nclass Foo { public function doFoo() {} }\nif ($x instanceof Foo) {\n    $x->";
        let d = doc(src);
        let pos = Position { line: 3, character: 8 };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None);
        let ls = labels(&items);
        assert!(ls.contains(&"doFoo"), "instanceof narrowing should make Foo methods available");
    }

    #[test]
    fn constructor_chain_arrow_completion() {
        let src = "<?php\nclass Builder { public function build() {} public function reset() {} }\n(new Builder())->";
        let d = doc(src);
        let pos = Position { line: 2, character: 16 };
        let items = filtered_completions_at(&d, &[], Some(">"), Some(src), Some(pos), None);
        let ls = labels(&items);
        assert!(ls.contains(&"build"), "constructor chain should complete Builder methods");
        assert!(ls.contains(&"reset"), "constructor chain should complete Builder methods");
    }
}
