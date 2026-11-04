// Reached only through the server, which is not built for wasm.
#![cfg_attr(target_family = "wasm", allow(dead_code))]

use std::sync::Arc;

use tower_lsp_server::ls_types::{Position, Uri, WorkspaceEdit};

use crate::document::ast::ParsedDoc;
use crate::lang::file_header::use_insertion;
use crate::types::resolve::{Declaration, resolve_declaration};

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
            return Some(format!(
                "{}\\{}",
                ns.trim_start_matches('\\'),
                decl.name.or_error()
            ));
        }
    }
    None
}

/// Build a `WorkspaceEdit` that inserts `use FQN;` into the namespace of the code at `at`.
pub(crate) fn build_use_import_edit(
    doc: &ParsedDoc,
    at: Position,
    uri: &Uri,
    fqn: &str,
) -> WorkspaceEdit {
    let edit = use_insertion(doc, at, |_| true).edit(&format!("use {fqn};"));
    crate::actions::single_file_edit(uri, vec![edit])
}

/// Build a `WorkspaceEdit` that inserts `use function FQN;` into the namespace of the code at `at`.
pub(crate) fn build_use_function_import_edit(
    doc: &ParsedDoc,
    at: Position,
    uri: &Uri,
    fqn: &str,
) -> WorkspaceEdit {
    let edit = use_insertion(doc, at, |_| true).edit(&format!("use function {fqn};"));
    crate::actions::single_file_edit(uri, vec![edit])
}
