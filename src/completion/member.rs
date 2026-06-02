use std::sync::Arc;

use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position};

use crate::ast::ParsedDoc;
use crate::stubs::builtin_class_members;
use crate::type_map::{
    ClassMembers, enclosing_class_at, is_backed_enum, is_enum, members_of_class, mixin_classes_of,
    parent_class_name,
};
use crate::util::{fqn_short_name, utf16_offset_to_byte};

use super::callable_item;

pub(super) fn all_instance_members(
    class_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    find_class_doc: Option<super::ClassDocLookup<'_>>,
) -> Vec<CompletionItem> {
    all_members(class_name, doc, other_docs, find_class_doc, false)
}

pub(super) fn all_static_members(
    class_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    find_class_doc: Option<super::ClassDocLookup<'_>>,
) -> Vec<CompletionItem> {
    all_members(class_name, doc, other_docs, find_class_doc, true)
}

/// Common class-hierarchy traversal for both instance (`->`) and static (`::`)
/// member completion. `is_static` selects which subset of members to surface:
/// - `false` (instance): non-static methods + properties, enum built-ins, mixins
/// - `true`  (static):   static methods + properties, constants
fn all_members(
    class_name: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    find_class_doc: Option<super::ClassDocLookup<'_>>,
    is_static: bool,
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
        let mut found_in_docs = false;

        // Fast path: workspace-index lookup gives O(1) access to the one doc
        // that defines `current`. Fallback: scan all docs linearly (original
        // behaviour, used when the index is unavailable or the class is not
        // yet indexed, e.g. built-ins or unsaved files).
        let fast_doc: Option<Arc<ParsedDoc>> = find_class_doc.and_then(|f| f(&current));
        let fast_ref: Option<&ParsedDoc> = fast_doc.as_deref();
        let defining: Option<(&ParsedDoc, ClassMembers)> = if let Some(fd) = fast_ref {
            let m = members_of_class(fd, &current);
            m.found.then_some((fd, m))
        } else {
            // PHP defines a class in exactly one file, so stop scanning once
            // the defining doc is hit.
            all.iter().find_map(|d| {
                let m = members_of_class(d, &current);
                m.found.then_some((*d, m))
            })
        };

        if let Some((d, members)) = defining {
            found_in_docs = true;
            parent = members.parent.clone();
            for (name, meth_is_static) in members.methods {
                if (meth_is_static == is_static) && seen_names.insert(name.clone()) {
                    // Method params unknown here; use has_params=true so
                    // snippet cursor lands inside parens.
                    items.push(callable_item(&name, CompletionItemKind::METHOD, true));
                }
            }
            for (name, prop_is_static) in &members.properties {
                if *prop_is_static == is_static {
                    let label = format!("${name}");
                    if seen_names.insert(label.clone()) {
                        let detail = if !is_static && members.readonly_properties.contains(name) {
                            Some("readonly".to_string())
                        } else {
                            None
                        };
                        items.push(CompletionItem {
                            label,
                            kind: Some(CompletionItemKind::PROPERTY),
                            detail,
                            ..Default::default()
                        });
                    }
                }
            }
            if is_static {
                for name in members.constants {
                    if seen_names.insert(name.clone()) {
                        items.push(CompletionItem {
                            label: name,
                            kind: Some(CompletionItemKind::CONSTANT),
                            ..Default::default()
                        });
                    }
                }
            } else {
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
                for mixin in mixin_classes_of(d, &current) {
                    queue.push(mixin);
                }
            }
            for trait_name in members.trait_uses {
                queue.push(trait_name);
            }
        }

        // Built-in stubs only apply when the class is not defined in any user
        // document — a user class shadowing a built-in name wins.
        if !found_in_docs && let Some(stub) = builtin_class_members(&current) {
            if parent.is_none() {
                parent = stub.parent.clone();
            }
            for (name, meth_is_static) in &stub.methods {
                if (*meth_is_static == is_static) && seen_names.insert(name.clone()) {
                    items.push(callable_item(name, CompletionItemKind::METHOD, true));
                }
            }
            for (name, prop_is_static) in &stub.properties {
                if *prop_is_static == is_static {
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
            if is_static {
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
    analysis: Option<&mir_analyzer::FileAnalysis>,
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
    // `before` now ends with the receiver variable; its last byte lands strictly
    // inside the (end-exclusive, identifier-only) mir symbol span.
    let var_offset = line_byte_offset(doc, position.line, before.len());
    if var_name == "$this" {
        return enclosing_class_at(source, doc, position)
            .or_else(|| analysis.and_then(|a| receiver_class_at(a, var_offset)));
    }
    // mir-primary; TypeMap covers gaps where mir records no class-typed symbol.
    analysis
        .and_then(|a| receiver_class_at(a, var_offset))
        .or_else(|| type_map.get(&var_name).map(str::to_owned))
}

/// Resolve the class(es) of a receiver variable from mir's recorded symbol at
/// `var_offset` (a byte offset inside the variable token), returning short
/// names for member lookup. A union receiver yields `Foo|Bar`. `None` if mir
/// recorded no class-typed symbol there.
pub(super) fn receiver_class_at(
    analysis: &mir_analyzer::FileAnalysis,
    var_offset: u32,
) -> Option<String> {
    let ty = crate::type_query::type_at_offset(analysis, var_offset)?;
    let names: Vec<String> = crate::type_query::class_names(ty)
        .iter()
        .map(|fqcn| fqn_short_name(fqcn).to_string())
        .collect();
    (!names.is_empty()).then(|| names.join("|"))
}

/// Doc byte offset of `byte_in_line` bytes into line `line`, minus one — i.e.
/// the last source byte of a token that ends at `byte_in_line`. Used to land an
/// offset strictly inside a receiver variable's end-exclusive mir span.
pub(super) fn line_byte_offset(doc: &ParsedDoc, line: u32, byte_in_line: usize) -> u32 {
    let line_start = doc.view().byte_of_position(Position { line, character: 0 });
    line_start + byte_in_line.saturating_sub(1) as u32
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
    Some(fqn_short_name(&class).to_string())
}
