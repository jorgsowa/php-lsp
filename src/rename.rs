use std::collections::HashMap;
use std::sync::Arc;

use php_parser_rs::parser::ast::Statement;
use tower_lsp::lsp_types::{
    Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::references::find_references_with_use;

/// Compute a WorkspaceEdit that renames every occurrence of `word` to `new_name`
/// across all open documents (including the declaration site).
pub fn rename(
    word: &str,
    new_name: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> WorkspaceEdit {
    let locations = find_references_with_use(word, all_docs, true);

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for loc in locations {
        changes.entry(loc.uri).or_default().push(TextEdit {
            range: loc.range,
            new_text: new_name.to_string(),
        });
    }

    WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }
}

/// Returns the range of the word at `position` if it's a renameable symbol.
/// Used for `textDocument/prepareRename`.
pub fn prepare_rename(source: &str, position: Position) -> Option<Range> {
    use crate::util::word_at;
    let word = word_at(source, position)?;
    // Only allow renaming plain identifiers (no sigil, no backslash namespace)
    if word.starts_with('$') || word.contains('\\') {
        return None;
    }
    let line = source.lines().nth(position.line as usize)?;
    let col = position.character as usize;
    // Find the start character of the word in UTF-16 offsets
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16_col >= col { break; }
        utf16_col += ch.len_utf16();
        char_idx += 1;
    }
    let mut left = char_idx;
    while left > 0 && is_word(chars[left - 1]) { left -= 1; }

    let start_utf16: u32 = chars[..left].iter().map(|c| c.len_utf16() as u32).sum();
    let end_utf16: u32 = start_utf16 + word.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
    Some(Range {
        start: Position { line: position.line, character: start_utf16 },
        end: Position { line: position.line, character: end_utf16 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ast(source: &str) -> Vec<Statement> {
        match php_parser_rs::parser::parse(source) {
            Ok(ast) => ast,
            Err(stack) => stack.partial,
        }
    }

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, source: &str) -> (Url, Arc<Vec<Statement>>) {
        (uri(path), Arc::new(parse_ast(source)))
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn rename_replaces_all_occurrences_in_single_file() {
        let src = "<?php\nfunction greet() {}\ngreet();\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let edit = rename("greet", "hello", &docs);
        let changes = edit.changes.unwrap();
        let edits = &changes[&uri("/a.php")];
        assert!(edits.len() >= 2, "expected at least 2 edits, got {}", edits.len());
        assert!(edits.iter().all(|e| e.new_text == "hello"));
    }

    #[test]
    fn rename_includes_declaration_site() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let edit = rename("greet", "hello", &docs);
        let changes = edit.changes.unwrap();
        let edits = &changes[&uri("/a.php")];
        // declaration (line 1) + call site (line 2)
        assert!(edits.len() >= 2, "should include declaration");
    }

    #[test]
    fn rename_across_files() {
        let a = doc("/a.php", "<?php\nfunction helper() {}");
        let b = doc("/b.php", "<?php\nhelper();");
        let docs = vec![a, b];
        let edit = rename("helper", "util", &docs);
        let changes = edit.changes.unwrap();
        assert!(changes.contains_key(&uri("/a.php")), "should rename declaration in a.php");
        assert!(changes.contains_key(&uri("/b.php")), "should rename usage in b.php");
    }

    #[test]
    fn prepare_rename_returns_word_range() {
        let src = "<?php\nfunction greet() {}";
        let result = prepare_rename(src, pos(1, 10));
        assert!(result.is_some(), "expected range for 'greet'");
        let range = result.unwrap();
        assert_eq!(range.start.line, 1);
    }

    #[test]
    fn prepare_rename_rejects_variables() {
        let src = "<?php\n$foo = 1;";
        let result = prepare_rename(src, pos(1, 1));
        assert!(result.is_none(), "should not allow renaming variables");
    }
}
