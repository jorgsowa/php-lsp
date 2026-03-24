use tower_lsp::lsp_types::{DocumentHighlight, DocumentHighlightKind, Position, Range};

use crate::ast::{ParsedDoc, offset_to_position};
use crate::util::word_at;
use crate::walk::refs_in_stmts;

/// Return all ranges in the document where the word at `position` appears.
pub fn document_highlights(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
) -> Vec<DocumentHighlight> {
    let word = match word_at(source, position) {
        Some(w) => w,
        None => return vec![],
    };

    let mut spans = Vec::new();
    refs_in_stmts(&doc.program().stmts, &word, &mut spans);

    spans
        .into_iter()
        .map(|span| {
            let start = offset_to_position(source, span.start);
            let end = Position {
                line: start.line,
                character: start.character + word.len() as u32,
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
        let src = "<?php\n$x = 1;";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 1));
        assert!(highlights.is_empty());
    }

    #[test]
    fn highlights_class_name() {
        let src = "<?php\nclass Foo {}\n$x = new Foo();";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 8));
        assert!(highlights.len() >= 2, "expected decl + new expr");
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
        assert!(
            highlights.len() >= 2,
            "expected at least 2 highlights, got: {}",
            highlights.len()
        );
    }

    #[test]
    fn no_highlights_beyond_line_end() {
        let src = "<?php\nfunction greet() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let highlights = document_highlights(src, &doc, pos(1, 999));
        assert!(highlights.is_empty());
    }
}
