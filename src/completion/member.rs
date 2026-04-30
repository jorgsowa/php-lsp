use std::sync::Arc;

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position};

use crate::ast::ParsedDoc;
use crate::stubs::builtin_class_members;
use crate::type_map::{
    enclosing_class_at, is_backed_enum, is_enum, members_of_class, mixin_classes_of,
    parent_class_name,
};
use crate::util::utf16_offset_to_byte;

use super::callable_item;

pub(super) fn all_instance_members(
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
        // PHP defines a class in exactly one file, so stop scanning once the
        // defining doc is hit. Without the early break, member completion
        // walks every workspace doc for every class in the inheritance chain.
        for d in &all {
            let members = members_of_class(d, &current);
            if !members.found {
                continue;
            }
            parent = members.parent.clone();
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
            break;
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

pub(super) fn all_static_members(
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
            if !members.found {
                continue;
            }
            parent = members.parent.clone();
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
            break;
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
pub(super) fn resolve_static_receiver(
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

pub(super) fn magic_method_completions() -> Vec<CompletionItem> {
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

pub(super) fn resolve_receiver_class(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    type_map: &crate::type_map::TypeMap,
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
