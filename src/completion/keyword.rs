use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

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
