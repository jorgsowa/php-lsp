use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, Position, Range};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::util::{utf16_pos_to_byte, word_at};
use crate::walk::{collect_var_refs_in_scope, refs_in_stmts};

/// Return all ranges in the document where the word at `position` appears.
/// For `$variables` the search is scope-aware: only occurrences within the
/// same function/method scope are returned, preventing unrelated variables
/// with the same name in other scopes from being highlighted.
pub fn document_highlights(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
) -> Vec<DocumentHighlight> {
    let word = match word_at(source, position) {
        Some(w) => w,
        None => return vec![],
    };

    let word_utf16_len: u32 = word.chars().map(|c| c.len_utf16() as u32).sum();
    let mut spans = Vec::new();
    let use_precise_end;

    if word.starts_with('$') {
        // Variable spans from collect_var_refs_in_scope are precise (cover exactly
        // `$varname`), so we can use span.end directly.
        let bare = word.trim_start_matches('$');
        let byte_off = utf16_pos_to_byte(source, position);
        collect_var_refs_in_scope(&doc.program().stmts, bare, byte_off, &mut spans);
        use_precise_end = true;
    } else {
        // refs_in_stmts pushes full statement spans for declarations (e.g. the
        // whole `function f() {}` node), so we compute end from word length.
        refs_in_stmts(&doc.program().stmts, &word, &mut spans);
        use_precise_end = false;
    }

    spans
        .into_iter()
        .map(|span| {
            let start = offset_to_position(source, span.start);
            let end = if use_precise_end {
                offset_to_position(source, span.end)
            } else {
                Position {
                    line: start.line,
                    character: start.character + word_utf16_len,
                }
            };
            DocumentHighlight {
                range: Range { start, end },
                kind: Some(DocumentHighlightKind::TEXT),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn highlights_function_declaration_and_calls() {
        let src = "<?php\nfunction greet() {}\ngreet();\ngreet();";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 10));
        assert_eq!(highlights.len(), 3, "decl + 2 calls: {:?}", highlights);
    }

    #[test]
    fn returns_empty_for_unknown_word() {
        let src = "<?php\necho 'hello';";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 6));
        assert!(highlights.is_empty());
    }

    #[test]
    fn highlights_variable_in_scope() {
        let src = "<?php\nfunction foo() {\n    $x = 1;\n    echo $x;\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(2, 5));
        assert_eq!(highlights.len(), 2, "should highlight both $x occurrences in foo()");
    }

    #[test]
    fn variable_highlight_does_not_cross_scope() {
        let src = "<?php\nfunction foo() { $x = 1; }\nfunction bar() { $x = 2; }";
        let doc = ParsedDoc::parse(src.to_string());
        // Cursor on $x in foo() — should NOT highlight $x in bar()
        let highlights = document_highlights(src, &doc, pos(1, 18));
        assert_eq!(highlights.len(), 1, "should only highlight $x within foo()");
    }

    #[test]
    fn highlights_class_name() {
        let src = "<?php\nclass Foo {}\n$x = new Foo();";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 8));
        assert_eq!(
            highlights.len(),
            2,
            "expected decl + new expr = 2 highlights"
        );
        let mut lines: Vec<u32> = highlights.iter().map(|h| h.range.start.line).collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            vec![1, 2],
            "Foo should be highlighted on lines 1 and 2"
        );
    }

    #[test]
    fn highlight_ranges_span_word_length() {
        let src = "<?php\nfunction greet() {}\ngreet();";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 10));
        for h in &highlights {
            let len = h.range.end.character - h.range.start.character;
            assert_eq!(len, "greet".len() as u32);
        }
    }

    #[test]
    fn highlights_method_calls() {
        let src = "<?php\nclass Calc { public function add() {} }\n$c = new Calc();\n$c->add();";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(3, 5));
        assert_eq!(
            highlights.len(),
            2,
            "expected exactly 2 highlights (decl + call), got: {}",
            highlights.len()
        );
        let mut lines: Vec<u32> = highlights.iter().map(|h| h.range.start.line).collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            vec![1, 3],
            "add() should be highlighted on lines 1 (decl) and 3 (call)"
        );
    }

    #[test]
    fn no_highlights_beyond_line_end() {
        let src = "<?php\nfunction greet() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 999));
        assert!(highlights.is_empty());
    }

    #[test]
    fn highlights_return_correct_count_and_lines() {
        // Symbol `myFn` used 3 times: declaration + 2 calls.
        let src = "<?php\nfunction myFn() {}\nmyFn();\nmyFn();";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 10));
        assert_eq!(
            highlights.len(),
            3,
            "expected exactly 3 highlights (decl + 2 calls), got: {:?}",
            highlights
                .iter()
                .map(|h| h.range.start.line)
                .collect::<Vec<_>>()
        );
        let mut lines: Vec<u32> = highlights.iter().map(|h| h.range.start.line).collect();
        lines.sort_unstable();
        assert_eq!(
            lines,
            vec![1, 2, 3],
            "myFn highlights should be on lines 1, 2, 3"
        );
    }
}
