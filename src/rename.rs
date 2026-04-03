use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp::lsp_types::{Position, Range, TextEdit, Url, WorkspaceEdit};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::references::find_references_with_use;
use crate::util::utf16_pos_to_byte;
use crate::walk::{collect_var_refs_in_scope, property_refs_in_stmts};

/// Compute a WorkspaceEdit that renames every occurrence of `word` to `new_name`
/// across all open documents (including the declaration site).
pub fn rename(word: &str, new_name: &str, all_docs: &[(Url, Arc<ParsedDoc>)]) -> WorkspaceEdit {
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
    if word.contains('\\') {
        return None;
    }
    let line = source.lines().nth(position.line as usize)?;
    let col = position.character as usize;
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16_col >= col {
            break;
        }
        utf16_col += ch.len_utf16();
        char_idx += 1;
    }
    let mut left = char_idx;
    while left > 0 && is_word(chars[left - 1]) {
        left -= 1;
    }

    let start_utf16: u32 = chars[..left].iter().map(|c| c.len_utf16() as u32).sum();
    let end_utf16: u32 = start_utf16 + word.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
    Some(Range {
        start: Position {
            line: position.line,
            character: start_utf16,
        },
        end: Position {
            line: position.line,
            character: end_utf16,
        },
    })
}

/// Rename a `$variable` (or parameter) within its enclosing function/method scope.
/// Only produces edits within the single document `uri`; variables don't cross files.
pub fn rename_variable(
    var_name: &str,
    new_name: &str,
    uri: &Url,
    source: &str,
    doc: &ParsedDoc,
    position: Position,
) -> WorkspaceEdit {
    let bare = var_name.trim_start_matches('$');
    let new_bare = new_name.trim_start_matches('$');
    let new_text = format!("${new_bare}");

    let byte_off = utf16_pos_to_byte(source, position);
    let stmts = &doc.program().stmts;

    let mut spans = Vec::new();
    collect_var_refs_in_scope(stmts, bare, byte_off, &mut spans);

    let mut edits: Vec<TextEdit> = spans
        .into_iter()
        .map(|span| {
            let start = offset_to_position(source, span.start);
            let end = offset_to_position(source, span.end);
            TextEdit {
                range: Range { start, end },
                new_text: new_text.clone(),
            }
        })
        .collect();

    edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));
    edits.dedup_by_key(|e| (e.range.start.line, e.range.start.character));

    let mut changes = HashMap::new();
    if !edits.is_empty() {
        changes.insert(uri.clone(), edits);
    }

    WorkspaceEdit {
        changes: if changes.is_empty() {
            None
        } else {
            Some(changes)
        },
        ..Default::default()
    }
}

/// Rename a property (`->prop` / `?->prop` / class declaration) across all indexed
/// documents.  Unlike variable rename, properties are not scope-bound and may appear
/// in many files.
pub fn rename_property(
    prop_name: &str,
    new_name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for (uri, doc) in all_docs {
        let source = doc.source();
        let mut spans = Vec::new();
        property_refs_in_stmts(source, &doc.program().stmts, prop_name, &mut spans);
        if !spans.is_empty() {
            let mut edits: Vec<TextEdit> = spans
                .into_iter()
                .map(|span| {
                    let start = offset_to_position(source, span.start);
                    let end = offset_to_position(source, span.end);
                    TextEdit {
                        range: Range { start, end },
                        new_text: new_name.to_string(),
                    }
                })
                .collect();
            edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));
            edits.dedup_by_key(|e| (e.range.start.line, e.range.start.character));
            changes.insert(uri.clone(), edits);
        }
    }
    WorkspaceEdit {
        changes: if changes.is_empty() {
            None
        } else {
            Some(changes)
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, source: &str) -> (Url, Arc<ParsedDoc>) {
        (uri(path), Arc::new(ParsedDoc::parse(source.to_string())))
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
        assert!(
            edits.len() >= 2,
            "expected at least 2 edits, got {}",
            edits.len()
        );
        assert!(edits.iter().all(|e| e.new_text == "hello"));
    }

    #[test]
    fn rename_includes_declaration_site() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let docs = vec![doc("/a.php", src)];
        let edit = rename("greet", "hello", &docs);
        let changes = edit.changes.unwrap();
        let edits = &changes[&uri("/a.php")];
        assert!(edits.len() >= 2, "should include declaration");
    }

    #[test]
    fn rename_across_files() {
        let a = doc("/a.php", "<?php\nfunction helper() {}");
        let b = doc("/b.php", "<?php\nhelper();");
        let docs = vec![a, b];
        let edit = rename("helper", "util", &docs);
        let changes = edit.changes.unwrap();
        assert!(
            changes.contains_key(&uri("/a.php")),
            "should rename declaration in a.php"
        );
        assert!(
            changes.contains_key(&uri("/b.php")),
            "should rename usage in b.php"
        );
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
    fn prepare_rename_rejects_fqn() {
        let src = "<?php\nFoo\\Bar::baz();";
        let result = prepare_rename(src, pos(1, 5));
        assert!(
            result.is_none(),
            "should not allow renaming FQNs with backslash"
        );
    }

    #[test]
    fn rename_does_not_match_partial_words() {
        // Renaming `foo` should not rename `foobar` or `barfoo`.
        let src = "<?php\nfunction foo() {}\nfunction foobar() {}\nfunction barfoo() {}\nfoo();\nfoobar();\nbarfoo();";
        let docs = vec![doc("/a.php", src)];
        let edit = rename("foo", "baz", &docs);
        let changes = edit.changes.unwrap();
        let edits = &changes[&uri("/a.php")];
        // Verify that every edit replaces exactly "foo" (not "foobar" or "barfoo")
        for e in edits {
            assert_eq!(e.new_text, "baz", "all edits should replace with 'baz'");
            let span_len = e.range.end.character - e.range.start.character;
            assert_eq!(
                span_len, 3,
                "renamed span should be length 3 (the word 'foo'), got {} at {:?}",
                span_len, e.range
            );
        }
        // Ensure that `foobar` and `barfoo` are not renamed: their line positions
        // should not appear in the edits.
        // Line 2 = `function foobar()`, line 3 = `function barfoo()`,
        // line 5 = `foobar()` call, line 6 = `barfoo()` call.
        let renamed_lines: Vec<u32> = edits.iter().map(|e| e.range.start.line).collect();
        assert!(
            !renamed_lines.contains(&5),
            "foobar() call (line 5) should not be renamed"
        );
        assert!(
            !renamed_lines.contains(&6),
            "barfoo() call (line 6) should not be renamed"
        );
    }

    #[test]
    fn rename_updates_use_statement() {
        // If file A defines `class Foo` and file B has `use Foo;`,
        // renaming `Foo` should update the use statement too.
        let a = doc("/a.php", "<?php\nclass Foo {}");
        let b = doc("/b.php", "<?php\nuse Foo;\n$x = new Foo();");
        let docs = vec![a, b];
        let edit = rename("Foo", "Bar", &docs);
        let changes = edit.changes.unwrap();

        // File a.php: the class declaration should be renamed.
        assert!(
            changes.contains_key(&uri("/a.php")),
            "should rename class declaration in a.php"
        );

        // File b.php: should have at least 2 edits — use statement + new expression.
        let b_edits = &changes[&uri("/b.php")];
        assert!(
            b_edits.len() >= 2,
            "expected at least 2 edits in b.php (use + new), got: {:?}",
            b_edits
        );
        assert!(
            b_edits.iter().all(|e| e.new_text == "Bar"),
            "all edits in b.php should rename to 'Bar'"
        );
        // One of the edits should be on the `use Foo;` line (line 1).
        let has_use_edit = b_edits.iter().any(|e| e.range.start.line == 1);
        assert!(
            has_use_edit,
            "expected an edit on the use statement line (line 1) in b.php"
        );
    }

    #[test]
    fn rename_variable_within_function() {
        let src = "<?php\nfunction foo() {\n    $x = 1;\n    echo $x;\n}";
        let doc = Arc::new(ParsedDoc::parse(src.to_string()));
        let edit = rename_variable("$x", "$y", &uri("/a.php"), src, &doc, pos(2, 5));
        let changes = edit.changes.unwrap();
        let edits = &changes[&uri("/a.php")];
        assert!(edits.len() >= 2, "should rename both assignment and usage");
        assert!(edits.iter().all(|e| e.new_text == "$y"));
    }

    #[test]
    fn rename_variable_does_not_cross_function_boundary() {
        let src = "<?php\nfunction foo() { $x = 1; }\nfunction bar() { $x = 2; }";
        let doc = Arc::new(ParsedDoc::parse(src.to_string()));
        let edit = rename_variable("$x", "$z", &uri("/a.php"), src, &doc, pos(1, 20));
        let changes = edit.changes.unwrap();
        let edits = &changes[&uri("/a.php")];
        // Only the $x in foo() should be renamed, not the one in bar()
        assert_eq!(edits.len(), 1, "should only rename within foo()");
    }

    #[test]
    fn prepare_rename_allows_variables() {
        let src = "<?php\n$foo = 1;";
        let result = prepare_rename(src, pos(1, 1));
        assert!(result.is_some(), "should allow renaming variables now");
    }

    #[test]
    fn rename_property_renames_declaration_and_accesses() {
        let src = "<?php\nclass Foo {\n    public string $name;\n    public function get() { return $this->name; }\n}";
        let docs = vec![doc("/a.php", src)];
        let edit = rename_property("name", "title", &docs);
        let changes = edit.changes.unwrap();
        let edits = &changes[&uri("/a.php")];
        assert!(edits.len() >= 2, "should rename declaration and access");
        assert!(edits.iter().all(|e| e.new_text == "title"));
    }

    #[test]
    fn rename_property_works_across_files() {
        let a = doc("/a.php", "<?php\nclass Foo {\n    public int $count;\n}");
        let b = doc("/b.php", "<?php\n$foo = new Foo();\necho $foo->count;");
        let docs = vec![a, b];
        let edit = rename_property("count", "total", &docs);
        let changes = edit.changes.unwrap();
        assert!(changes.contains_key(&uri("/a.php")), "declaration in a.php");
        assert!(changes.contains_key(&uri("/b.php")), "access in b.php");
    }
}
