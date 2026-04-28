use php_ast::{ExprKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

/// An `#[\Attribute]`-annotated class with its resolved target bitmask.
///
/// Target bitmask uses PHP's `Attribute::TARGET_*` constants:
/// - `TARGET_CLASS = 1`
/// - `TARGET_FUNCTION = 2`
/// - `TARGET_METHOD = 4`
/// - `TARGET_PROPERTY = 8`
/// - `TARGET_CLASS_CONSTANT = 16`
/// - `TARGET_PARAMETER = 32`
/// - `TARGET_ALL = 63` (default when no argument is given)
pub(super) struct AttributeClassEntry {
    pub label: String,
    pub fqn: String,
    pub target: i64,
}

/// Collect only classes annotated with `#[\Attribute]` (or `#[Attribute]`),
/// resolving the first constructor argument as the target bitmask.
pub(super) fn collect_attribute_classes(
    stmts: &[Stmt<'_, '_>],
    ns_prefix: &str,
    out: &mut Vec<AttributeClassEntry>,
) {
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
                if short.is_empty() {
                    continue;
                }
                let target = attribute_target_from_attrs(&c.attributes);
                if let Some(target) = target {
                    out.push(AttributeClassEntry {
                        label: short.to_string(),
                        fqn: fqn_for(short, &cur_ns),
                        target,
                    });
                }
            }
            StmtKind::Namespace(ns) => {
                let ns_name = ns
                    .name
                    .as_ref()
                    .map(|n| n.to_string_repr().to_string())
                    .unwrap_or_default();
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        collect_attribute_classes(inner, &ns_name, out);
                    }
                    NamespaceBody::Simple => {
                        cur_ns = ns_name;
                    }
                }
            }
            _ => {}
        }
    }
}

/// Returns `Some(target_bitmask)` if the attribute list contains an
/// `#[Attribute]` or `#[\Attribute]` annotation, `None` otherwise.
///
/// The target is parsed from the first argument:
/// - `#[\Attribute]` → `Some(63)` (TARGET_ALL)
/// - `#[\Attribute(63)]` → `Some(63)`
/// - `#[\Attribute(\Attribute::TARGET_CLASS)]` → `Some(1)`
fn attribute_target_from_attrs(
    attrs: &php_ast::ArenaVec<'_, php_ast::Attribute<'_, '_>>,
) -> Option<i64> {
    for attr in attrs.iter() {
        let name = attr.name.to_string_repr();
        if name.rsplit('\\').next() != Some("Attribute") {
            continue;
        }
        // Found the `#[Attribute]` annotation; parse its target argument.
        let target = attr
            .args
            .first()
            .and_then(|arg| resolve_target_expr(&arg.value.kind))
            .unwrap_or(63); // TARGET_ALL when no argument given
        return Some(target);
    }
    None
}

fn resolve_target_expr(expr: &ExprKind<'_, '_>) -> Option<i64> {
    match expr {
        ExprKind::Int(v) => Some(*v),
        ExprKind::ClassConstAccess(acc) => {
            // `\Attribute::TARGET_CLASS` → member is an Identifier "TARGET_CLASS"
            acc.member.name_str().and_then(target_const_to_bitmask)
        }
        _ => None,
    }
}

fn target_const_to_bitmask(name: &str) -> Option<i64> {
    match name {
        "TARGET_CLASS" => Some(1),
        "TARGET_FUNCTION" => Some(2),
        "TARGET_METHOD" => Some(4),
        "TARGET_PROPERTY" => Some(8),
        "TARGET_CLASS_CONSTANT" => Some(16),
        "TARGET_PARAMETER" => Some(32),
        "TARGET_ALL" => Some(63),
        _ => None,
    }
}

/// Infer the attribute target context by looking ahead from `position` in
/// `source` for the first declaration keyword (class / function / etc.).
///
/// Returns the appropriate `Attribute::TARGET_*` bitmask, or `63` (ALL) if
/// no specific context can be determined.
pub(super) fn infer_attribute_target(source: &str, position: Position) -> i64 {
    let lines: Vec<&str> = source.lines().collect();
    let start = (position.line as usize).saturating_add(1);
    for line in lines.iter().skip(start).take(10) {
        let t = line.trim();
        if t.is_empty()
            || t.starts_with("//")
            || t.starts_with("/*")
            || t.starts_with("*")
            || t.starts_with("#[")
        {
            continue;
        }
        // Strip common visibility/modifier keywords before checking
        let stripped = t
            .trim_start_matches("abstract ")
            .trim_start_matches("final ")
            .trim_start_matches("readonly ")
            .trim_start_matches("public ")
            .trim_start_matches("protected ")
            .trim_start_matches("private ")
            .trim_start_matches("static ");
        if stripped.starts_with("class ")
            || stripped.starts_with("interface ")
            || stripped.starts_with("enum ")
            || stripped.starts_with("trait ")
        {
            return 1; // TARGET_CLASS
        }
        if stripped.starts_with("function ") {
            return 2 | 4; // TARGET_FUNCTION | TARGET_METHOD
        }
        // Property: starts with a type hint or $ before any `=`
        if stripped.starts_with('$') || is_property_declaration(stripped) {
            return 8; // TARGET_PROPERTY
        }
        if stripped.starts_with("const ") {
            return 16; // TARGET_CLASS_CONSTANT
        }
        break;
    }
    63 // TARGET_ALL — context unknown
}

fn is_property_declaration(s: &str) -> bool {
    // Heuristic: PHP type followed by `$varname`
    // e.g. `string $name`, `int $count`, `?Foo $bar`
    let s = s.trim_start_matches('?');
    if let Some(rest) = s.split_once(' ') {
        rest.1.trim().starts_with('$')
    } else {
        false
    }
}

pub(super) fn collect_classes_with_ns(
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
pub(super) fn use_insert_position(source: &str) -> Position {
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
pub(super) fn current_file_namespace(stmts: &[Stmt<'_, '_>]) -> String {
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

/// Collect fully-qualified names from stmts that contain `prefix`.
pub(super) fn collect_fqns_with_prefix(
    stmts: &[Stmt<'_, '_>],
    ns: &str,
    prefix: &str,
    out: &mut Vec<CompletionItem>,
) {
    let prefix_lc = prefix.to_lowercase();
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                if let Some(name) = c.name {
                    let fqn = if ns.is_empty() {
                        name.to_string()
                    } else {
                        format!("{ns}\\{name}")
                    };
                    if prefix.is_empty() || fqn.to_lowercase().contains(&prefix_lc) {
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
                if prefix.is_empty() || fqn.to_lowercase().contains(&prefix_lc) {
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

/// Returns the prefix typed after `use ` on the current line, or None if not in a use statement.
pub(super) fn use_completion_prefix(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = crate::util::utf16_offset_to_byte(line, position.character as usize);
    let before = line[..col].trim_start();
    let prefix = before.strip_prefix("use ")?;
    Some(prefix.trim_start_matches('\\').to_string())
}

/// Extract the identifier characters typed immediately before the cursor.
/// Includes `\` to support namespace-qualified prefixes like `App\Serv`.
pub(super) fn typed_prefix(source: Option<&str>, position: Option<Position>) -> Option<String> {
    let src = source?;
    let pos = position?;
    let line = src.lines().nth(pos.line as usize)?;
    let col = crate::util::utf16_offset_to_byte(line, pos.character as usize);
    let before = &line[..col];
    let prefix: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '\\' || c == '$')
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
