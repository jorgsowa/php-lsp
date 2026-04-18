use php_ast::{NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

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
                    if fqn.to_lowercase().contains(&prefix_lc) || prefix.is_empty() {
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
                if fqn.to_lowercase().contains(&prefix_lc) || prefix.is_empty() {
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
