use std::sync::Arc;

use tower_lsp_server::ls_types::{CompletionItem, CompletionItemKind, Position};

use super::member::{line_byte_offset, receiver_class_at};
use crate::document::ast::ParsedDoc;
use crate::types::type_map::{enclosing_class_at, members_of_class};

pub(super) fn match_arm_completions(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[Arc<ParsedDoc>],
    position: Position,
    analysis: Option<&mir_analyzer::FileAnalysis>,
    find_class_doc: Option<super::ClassDocLookup<'_>>,
) -> Option<Vec<CompletionItem>> {
    let start_line = position.line as usize;
    let end_line = start_line.saturating_sub(5);
    let all_lines: Vec<&str> = source.lines().collect();
    for line_idx in (end_line..=start_line).rev() {
        let line = all_lines.get(line_idx).copied()?;
        if let Some(cap) = extract_match_subject(line) {
            let class_name = if cap == "this" {
                enclosing_class_at(source, doc, position)?
            } else {
                // Resolve the match subject `$cap` via mir at its position on
                // this line (`$cap` always appears on the matched line).
                let subject_byte = line.find(&format!("${cap}"))?;
                let var_offset = line_byte_offset(doc, line_idx as u32, subject_byte + 1);
                analysis.and_then(|a| receiver_class_at(a, var_offset))?
            };
            // Fast path: workspace-index lookup gives O(1) access to the one doc
            // that defines `class_name`. Fallback: scan all docs linearly (used
            // when the index is unavailable or the class isn't indexed yet).
            let fast_doc = find_class_doc.and_then(|f| f(&class_name));
            let members = if let Some(fd) = &fast_doc {
                Some(members_of_class(fd, &class_name))
            } else {
                let all_docs: Vec<&ParsedDoc> = std::iter::once(doc)
                    .chain(other_docs.iter().map(|d| d.as_ref()))
                    .collect();
                all_docs.iter().find_map(|d| {
                    let m = members_of_class(d, &class_name);
                    (!m.constants.is_empty()).then_some(m)
                })
            };
            if let Some(members) = members
                && !members.constants.is_empty()
            {
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
    None
}

pub(super) fn extract_match_subject(line: &str) -> Option<String> {
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
