use php_ast::{
    ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, NamespaceDecl, Param, Stmt, StmtKind,
};
use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind};

use crate::document::ast::{ParsedDoc, SourceView};

use super::{callable_item, named_arg_item, named_arg_label};

/// Predicate over an item's label and kind, checked before the item is built.
pub type SymbolFilter<'a> = &'a dyn Fn(&str, CompletionItemKind) -> bool;

pub fn symbol_completions(doc: &ParsedDoc) -> Vec<CompletionItem> {
    symbol_completions_matching(doc, &|_, _| true)
}

/// Like [`symbol_completions`] but only materializes items passing `keep`.
/// Callables carry no signature or docs: `completion_resolve` fills them from
/// the FQN hint in `data`, so a large unfiltered list stays cheap.
pub fn symbol_completions_matching(doc: &ParsedDoc, keep: SymbolFilter<'_>) -> Vec<CompletionItem> {
    let mut c = Collector::new(keep);
    c.statements(&doc.program().stmts);
    c.items
}

/// Like [`symbol_completions_matching`] but only includes variables declared
/// at or before `line`. Non-variable items (functions, classes, etc.) are always included.
pub fn symbol_completions_before_matching(
    doc: &ParsedDoc,
    line: u32,
    keep: SymbolFilter<'_>,
) -> Vec<CompletionItem> {
    let mut c = Collector::new(keep);
    c.statements_before(&doc.program().stmts, line, doc.view());
    c.items
}

struct Collector<'k> {
    items: Vec<CompletionItem>,
    keep: SymbolFilter<'k>,
    namespace: String,
    class_fqn: Option<String>,
}

impl<'k> Collector<'k> {
    fn new(keep: SymbolFilter<'k>) -> Self {
        Collector {
            items: Vec::new(),
            keep,
            namespace: String::new(),
            class_fqn: None,
        }
    }

    fn qualify(&self, short: &str) -> String {
        if self.namespace.is_empty() {
            short.to_string()
        } else {
            format!("{}\\{}", self.namespace, short)
        }
    }

    /// Enters `ns`: a braced body is walked by `walk` under that namespace,
    /// a simple `namespace Foo;` applies to the statements that follow.
    fn namespace<'a, 'b>(
        &mut self,
        ns: &NamespaceDecl<'a, 'b>,
        walk: impl FnOnce(&mut Self, &[Stmt<'a, 'b>]),
    ) {
        let name = ns
            .name
            .as_ref()
            .map(|n| n.to_string_repr().to_string())
            .unwrap_or_default();
        match &ns.body {
            NamespaceBody::Braced(inner) => {
                let outer = std::mem::replace(&mut self.namespace, name);
                walk(self, &inner.stmts);
                self.namespace = outer;
            }
            NamespaceBody::Simple => self.namespace = name,
        }
    }

    fn push(&mut self, label: String, kind: CompletionItemKind) {
        if (self.keep)(&label, kind) {
            self.items.push(CompletionItem {
                label,
                kind: Some(kind),
                ..Default::default()
            });
        }
    }

    fn push_variable(&mut self, name: &str) {
        if name != "this" {
            self.push(format!("${name}"), CompletionItemKind::VARIABLE);
        }
    }

    fn push_callable(&mut self, name: &str, kind: CompletionItemKind, params: &[Param<'_, '_>]) {
        if (self.keep)(name, kind) {
            let mut item = callable_item(name, kind, !params.is_empty());
            item.data = Some(match &self.class_fqn {
                Some(class) => serde_json::json!({ "class": class }),
                None => serde_json::json!({ "fqn": self.qualify(name) }),
            });
            self.items.push(item);
        }
        if !params.is_empty() {
            let label = named_arg_label(name, params);
            if (self.keep)(&label, kind) {
                self.items.push(named_arg_item(label, kind, params));
            }
        }
    }

    fn statements_before(&mut self, stmts: &[Stmt<'_, '_>], line: u32, sv: SourceView<'_>) {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Expression(e) => {
                    // Only add variables if they appear at or before the cursor line
                    let stmt_line = sv.position_of(stmt.span.start).line;
                    if stmt_line <= line {
                        self.expression(e);
                    }
                }
                StmtKind::Namespace(ns) => {
                    self.namespace(ns, |c, inner| c.statements_before(inner, line, sv));
                }
                // Non-variable items: always include
                _ => self.statements(std::slice::from_ref(stmt)),
            }
        }
    }

    fn statements(&mut self, stmts: &[Stmt<'_, '_>]) {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Function(f) => {
                    self.push_callable(
                        &f.name.to_string(),
                        CompletionItemKind::FUNCTION,
                        &f.params,
                    );
                    for param in f.params.iter() {
                        self.push(format!("${}", param.name), CompletionItemKind::VARIABLE);
                    }
                }
                StmtKind::Class(c) => {
                    let short = c.name.as_ref().map(|n| n.to_string());
                    if let Some(short) = &short {
                        self.push(short.clone(), CompletionItemKind::CLASS);
                    }
                    self.class_fqn = short.map(|s| self.qualify(&s));
                    for member in c.body.members.iter() {
                        match &member.kind {
                            ClassMemberKind::Method(m) => self.push_callable(
                                &m.name.to_string(),
                                CompletionItemKind::METHOD,
                                &m.params,
                            ),
                            ClassMemberKind::Property(p) => {
                                self.push(format!("${}", p.name), CompletionItemKind::PROPERTY);
                            }
                            ClassMemberKind::ClassConst(c) => {
                                self.push(c.name.to_string(), CompletionItemKind::CONSTANT);
                            }
                            _ => {}
                        }
                    }
                    self.class_fqn = None;
                }
                StmtKind::Interface(i) => {
                    self.push(i.name.to_string(), CompletionItemKind::INTERFACE);
                }
                StmtKind::Trait(t) => self.push(t.name.to_string(), CompletionItemKind::CLASS),
                StmtKind::Enum(e) => {
                    self.push(e.name.to_string(), CompletionItemKind::ENUM);
                    for member in e.body.members.iter() {
                        if let EnumMemberKind::Case(c) = &member.kind {
                            self.push(
                                format!("{}::{}", e.name, c.name),
                                CompletionItemKind::ENUM_MEMBER,
                            );
                        }
                    }
                }
                StmtKind::Namespace(ns) => self.namespace(ns, |c, inner| c.statements(inner)),
                StmtKind::Expression(e) => self.expression(e),
                _ => {}
            }
        }
    }

    fn expression(&mut self, expr: &php_ast::Expr<'_, '_>) {
        if let ExprKind::Assign(assign) = &expr.kind {
            match &assign.target.kind {
                ExprKind::Variable(name) => self.push_variable(name.as_str()),
                // Array destructuring: [$a, $b] = ... or list($a, $b) = ...
                ExprKind::Array(elements) => {
                    for elem in elements.iter() {
                        // Variables can be in the value (for indexed arrays) or as value/key pairs
                        if let ExprKind::Variable(name) = &elem.value.kind {
                            self.push_variable(name.as_str());
                        }
                        // Also check key (for associative destructuring like [key => $var])
                        if let Some(key) = &elem.key
                            && let ExprKind::Variable(name) = &key.kind
                        {
                            self.push_variable(name.as_str());
                        }
                    }
                }
                _ => {}
            }
            self.expression(assign.value);
        }
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
    "sleep",
    "usleep",
    "exit",
    "die",
];

pub fn builtin_completions() -> Vec<CompletionItem> {
    builtin_completions_matching(&|_| true)
}

/// Like [`builtin_completions`] but only materializes items whose label
/// passes `keep` — completion builds these per request, and constructing
/// hundreds of builtin items just for a prefix retain to drop them dominated
/// the request's allocations.
pub fn builtin_completions_matching(keep: &dyn Fn(&str) -> bool) -> Vec<CompletionItem> {
    let mut seen = std::collections::HashSet::new();
    PHP_BUILTINS
        .iter()
        .filter(|&&f| keep(f) && seen.insert(f))
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
    superglobal_completions_matching(&|_| true)
}

pub fn superglobal_completions_matching(keep: &dyn Fn(&str) -> bool) -> Vec<CompletionItem> {
    PHP_SUPERGLOBALS
        .iter()
        .filter(|name| keep(name))
        .map(|&name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::VARIABLE),
            detail: Some("superglobal".to_string()),
            ..Default::default()
        })
        .collect()
}
