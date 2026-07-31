use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind, Position, Range, TextEdit};

use super::namespace::{
    collect_attribute_classes, current_file_namespace, infer_attribute_target, use_insert_position,
};
use crate::document::ast::ParsedDoc;

/// Build attribute-class completion items for a `#[` context.
///
/// Only classes annotated with `#[\Attribute]` are included. If the
/// look-ahead finds a specific declaration (class / function / property),
/// items are further filtered by the matching `TARGET_*` bitmask.
pub(super) fn attribute_completions(
    source: &str,
    position: Position,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    imports: &HashMap<String, String>,
) -> Vec<CompletionItem> {
    let context_target = infer_attribute_target(source, position);
    let cur_ns = current_file_namespace(&doc.program().stmts);
    let mut items: Vec<CompletionItem> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Current doc — no auto-import needed.
    let mut cur_entries = Vec::new();
    collect_attribute_classes(&doc.program().stmts, "", &mut cur_entries);
    for entry in cur_entries {
        if entry.target & context_target == 0 {
            continue;
        }
        if seen.insert(entry.label.clone()) {
            items.push(CompletionItem {
                label: entry.label,
                kind: Some(CompletionItemKind::CLASS),
                ..Default::default()
            });
        }
    }

    // Other docs — add `use` statement when crossing namespaces.
    for other in other_docs {
        let mut entries = Vec::new();
        collect_attribute_classes(&other.program().stmts, "", &mut entries);
        for entry in entries {
            if entry.target & context_target == 0 {
                continue;
            }
            if !seen.insert(entry.label.clone()) {
                continue;
            }
            let in_same_ns =
                !cur_ns.is_empty() && entry.fqn == format!("{}\\{}", cur_ns, entry.label);
            let is_global = !entry.fqn.contains('\\');
            let already = imports.contains_key(&entry.label);
            let additional_text_edits = if !in_same_ns && !is_global && !already {
                let insert_pos = use_insert_position(source);
                Some(vec![TextEdit {
                    range: Range {
                        start: insert_pos,
                        end: insert_pos,
                    },
                    new_text: format!("use {};\n", entry.fqn),
                }])
            } else {
                None
            };
            items.push(CompletionItem {
                label: entry.label,
                kind: Some(CompletionItemKind::CLASS),
                detail: if entry.fqn.contains('\\') {
                    Some(entry.fqn)
                } else {
                    None
                },
                additional_text_edits,
                ..Default::default()
            });
        }
    }
    items
}
