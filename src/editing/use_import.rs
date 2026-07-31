use std::collections::HashMap;

use tower_lsp_server::ls_types::{Position, Range, TextEdit, Uri, WorkspaceEdit};

use crate::index::file_index::FileIndex;

/// Find a class FQN matching `name` in the workspace indexes. Unlike
/// `find_fqn_for_function`, a match is returned even when the class lives in
/// the global namespace (`fqn` has no `\`): referencing a global-namespace
/// class from inside a namespaced file still requires an explicit `use`,
/// since (unlike functions/constants) unqualified class names never fall
/// back to the global namespace.
pub(crate) fn find_fqn_for_class(
    name: &str,
    indexes: &[(Uri, std::sync::Arc<FileIndex>)],
) -> Option<String> {
    for (_uri, idx) in indexes {
        for class in &idx.classes {
            if class.name.as_ref() == name {
                return Some(class.fqn.to_string());
            }
        }
    }
    None
}

/// Build a `WorkspaceEdit` that inserts `use FQN;` near the top of the file.
pub(crate) fn build_use_import_edit(source: &str, uri: &Uri, fqn: &str) -> WorkspaceEdit {
    // Insert after the `<?php` line and any existing `use` / `namespace` lines
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

/// Find a namespaced function FQN matching `name` in the workspace indexes.
/// Returns `Some(fqn)` only when the FQN is namespaced (contains `\`).
pub(crate) fn find_fqn_for_function(
    name: &str,
    indexes: &[(Uri, std::sync::Arc<FileIndex>)],
) -> Option<String> {
    for (_uri, idx) in indexes {
        for func in &idx.functions {
            if func.name.as_ref() == name && func.fqn.contains('\\') {
                return Some(func.fqn.trim_start_matches('\\').to_string());
            }
        }
    }
    None
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
