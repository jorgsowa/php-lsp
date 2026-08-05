use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp_server::ls_types::{Position, Range, TextEdit, Uri, WorkspaceEdit};

use crate::document::ast::ParsedDoc;
use crate::types::resolve::{Declaration, resolve_declaration};

pub(crate) fn find_fqn_for_class(
    name: &str,
    class_candidates: &dyn Fn(&str) -> Vec<crate::db::workspace_index::ClassRef>,
    resolve_class_fqn: &dyn Fn(crate::db::workspace_index::ClassRef) -> Option<String>,
) -> Option<String> {
    class_candidates(name)
        .into_iter()
        .find_map(resolve_class_fqn)
}

pub(crate) fn find_fqn_for_function(
    name: &str,
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
    function_candidates: &dyn Fn(&str) -> Vec<Uri>,
) -> Option<String> {
    for uri in function_candidates(name) {
        let Some(doc) = get_doc(&uri) else { continue };
        let Some(decl) = resolve_declaration(&doc.program().stmts, name, &|decl| {
            matches!(decl, Declaration::Function { .. })
        }) else {
            continue;
        };
        if let Declaration::Function { decl, .. } = decl {
            let ns = doc
                .program()
                .stmts
                .iter()
                .find_map(|stmt| {
                    if let php_ast::StmtKind::Namespace(ns) = &stmt.kind {
                        ns.name.as_ref().map(|n| n.to_string_repr().to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            if ns.is_empty() {
                continue;
            }
            return Some(format!("{}\\{}", ns.trim_start_matches('\\'), decl.name.or_error()));
        }
    }
    None
}

/// Build a `WorkspaceEdit` that inserts `use FQN;` near the top of the file.
pub(crate) fn build_use_import_edit(source: &str, uri: &Uri, fqn: &str) -> WorkspaceEdit {
    let insert_line = find_use_insert_line(source);
    let insert_text = format!("use {fqn};\n");
    let pos = Position {
        line: insert_line,
        character: 0,
    };
    let edit = TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text: insert_text,
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }
}

/// Build a `WorkspaceEdit` that inserts `use function FQN;` near the top of the file.
pub(crate) fn build_use_function_import_edit(source: &str, uri: &Uri, fqn: &str) -> WorkspaceEdit {
    let insert_line = find_use_insert_line(source);
    let insert_text = format!("use function {fqn};\n");
    let pos = Position {
        line: insert_line,
        character: 0,
    };
    let edit = TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text: insert_text,
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }
}

pub(crate) fn find_use_insert_line(source: &str) -> u32 {
    let mut last_use_or_ns: u32 = 0;
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("<?php")
            || trimmed.starts_with("namespace ")
            || trimmed.starts_with("use ")
        {
            last_use_or_ns = i as u32 + 1;
        }
    }
    last_use_or_ns
}
