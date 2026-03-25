use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp::lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};

use crate::ast::ParsedDoc;

/// Build a `WorkspaceEdit` that updates every `use` import referencing `old_fqn`
/// to `new_fqn` across all indexed documents.
pub fn use_edits_for_rename(
    old_fqn: &str,
    new_fqn: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> WorkspaceEdit {
    if old_fqn == new_fqn {
        return WorkspaceEdit::default();
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    for (uri, doc) in all_docs {
        let edits = use_edits_in_source(doc.source(), old_fqn, new_fqn);
        if !edits.is_empty() {
            changes.insert(uri.clone(), edits);
        }
    }

    WorkspaceEdit {
        changes: if changes.is_empty() { None } else { Some(changes) },
        ..Default::default()
    }
}

/// Scan `source` for `use` statements that reference `old_fqn` and return
/// `TextEdit`s that replace `old_fqn` with `new_fqn` in each such line.
///
/// Handles:
/// - `use OldFqn;`
/// - `use \OldFqn;`
/// - `use OldFqn as Alias;`
fn use_edits_in_source(source: &str, old_fqn: &str, new_fqn: &str) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    let old = old_fqn.trim_start_matches('\\');
    let new_clean = new_fqn.trim_start_matches('\\');

    for (line_idx, line) in source.lines().enumerate() {
        // Only process use-statement lines
        let trimmed = line.trim_start();
        if !trimmed.starts_with("use ") {
            continue;
        }

        let Some(use_pos) = line.find("use ") else { continue };
        let after_use = use_pos + 4; // byte offset right after "use "

        // Skip an optional leading backslash in the source
        let (fqn_start, fqn_str) = if line.as_bytes().get(after_use) == Some(&b'\\') {
            (after_use + 1, &line[after_use + 1..])
        } else {
            (after_use, &line[after_use..])
        };

        if !fqn_str.starts_with(old) {
            continue;
        }

        // Confirm the match ends on a word boundary (`;`, space, `{`, `,`, end-of-string)
        let after_fqn = &fqn_str[old.len()..];
        let is_boundary = after_fqn.is_empty()
            || matches!(after_fqn.as_bytes()[0], b';' | b' ' | b'\t' | b'{' | b',');
        if !is_boundary {
            continue;
        }

        let line_u32 = line_idx as u32;
        edits.push(TextEdit {
            range: Range {
                start: Position {
                    line: line_u32,
                    character: fqn_start as u32,
                },
                end: Position {
                    line: line_u32,
                    character: (fqn_start + old.len()) as u32,
                },
            },
            new_text: new_clean.to_string(),
        });
    }

    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Arc<ParsedDoc> {
        Arc::new(ParsedDoc::parse(src.to_string()))
    }

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    #[test]
    fn replaces_simple_use_statement() {
        let src = "<?php\nuse App\\Services\\Foo;\n";
        let edits = use_edits_in_source(src, "App\\Services\\Foo", "App\\Services\\Bar");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "App\\Services\\Bar");
        assert_eq!(edits[0].range.start.line, 1);
    }

    #[test]
    fn replaces_use_with_leading_backslash() {
        let src = "<?php\nuse \\App\\Services\\Foo;\n";
        let edits = use_edits_in_source(src, "App\\Services\\Foo", "App\\Other\\Foo");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "App\\Other\\Foo");
    }

    #[test]
    fn replaces_use_with_alias() {
        let src = "<?php\nuse App\\Services\\Foo as F;\n";
        let edits = use_edits_in_source(src, "App\\Services\\Foo", "App\\Services\\Bar");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "App\\Services\\Bar");
    }

    #[test]
    fn does_not_replace_partial_match() {
        // App\Services\FooExtra should NOT be replaced when old is App\Services\Foo
        let src = "<?php\nuse App\\Services\\FooExtra;\n";
        let edits = use_edits_in_source(src, "App\\Services\\Foo", "App\\Services\\Bar");
        assert_eq!(edits.len(), 0);
    }

    #[test]
    fn no_edits_when_fqn_unchanged() {
        let docs = vec![(uri("/a.php"), doc("<?php\nuse App\\Foo;\n"))];
        let edit = use_edits_for_rename("App\\Foo", "App\\Foo", &docs);
        assert!(edit.changes.is_none());
    }

    #[test]
    fn edits_span_multiple_files() {
        let docs = vec![
            (uri("/a.php"), doc("<?php\nuse App\\Old;\n")),
            (uri("/b.php"), doc("<?php\nuse App\\Old;\n")),
            (uri("/c.php"), doc("<?php\nuse App\\Other;\n")),
        ];
        let edit = use_edits_for_rename("App\\Old", "App\\New", &docs);
        let changes = edit.changes.unwrap();
        assert!(changes.contains_key(&uri("/a.php")));
        assert!(changes.contains_key(&uri("/b.php")));
        assert!(!changes.contains_key(&uri("/c.php")));
    }

    #[test]
    fn ignores_non_use_lines() {
        let src = "<?php\n// use App\\Old;\n$x = new App\\Old();\n";
        let edits = use_edits_in_source(src, "App\\Old", "App\\New");
        assert_eq!(edits.len(), 0, "should only touch use statements");
    }
}
