use tower_lsp_server::ls_types::{DocumentHighlight, DocumentHighlightKind, Position, Range};

use crate::document::ast::ParsedDoc;
use crate::lang::is_unresolvable_bareword_at;
use crate::navigation::walk::{collect_var_refs_in_scope, refs_in_stmts};
use crate::text::word_at_position;

/// Return all ranges in the document where the word at `position` appears.
/// For `$variables` the search is scope-aware: only occurrences within the
/// same function/method scope are returned, preventing unrelated variables
/// with the same name in other scopes from being highlighted.
pub fn document_highlights(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
) -> Vec<DocumentHighlight> {
    let word = match word_at_position(source, position) {
        Some(w) => w,
        None => return vec![],
    };

    // A bare keyword (`final`, `public`, ...) is never a real reference —
    // `refs_in_stmts` below matches by bare name with no keyword awareness,
    // so a modifier token would otherwise highlight unrelated call/reference
    // sites of a same-named method elsewhere in the file (semi-reserved
    // words are valid method names). Also feeds `linked_editing_range`,
    // which calls this function directly.
    if is_unresolvable_bareword_at(source, position, &word) {
        return vec![];
    }

    let word_utf16_len: u32 = word.chars().map(|c| c.len_utf16() as u32).sum();
    let sv = doc.view();

    if word.starts_with('$') {
        let bare = word.trim_start_matches('$');
        let byte_off = sv.byte_of_position(position) as usize;
        let mut var_spans = Vec::new();
        collect_var_refs_in_scope(&doc.program().stmts, bare, byte_off, &mut var_spans);
        var_spans
            .into_iter()
            .map(|(span, kind)| {
                let start = sv.position_of(span.start);
                let end = Position {
                    line: start.line,
                    character: start.character + word_utf16_len,
                };
                DocumentHighlight {
                    range: Range { start, end },
                    kind: Some(kind),
                }
            })
            .collect()
    } else {
        // Use `doc.source()` (the string the AST was parsed from), not the
        // caller's `source`. `refs_in_stmts` resolves AST name slices via
        // `str_offset` pointer arithmetic; if the parameter `source` is a
        // separate allocation, the arithmetic falls back to a content
        // search that returns the *first* textual occurrence — including
        // hits inside comments and string literals — instead of the actual
        // AST node location.
        let mut spans = Vec::new();
        refs_in_stmts(doc.source(), &doc.program().stmts, &word, &mut spans);
        spans
            .into_iter()
            .map(|span| {
                let start = sv.position_of(span.start);
                let end = Position {
                    line: start.line,
                    character: start.character + word_utf16_len,
                };
                DocumentHighlight {
                    range: Range { start, end },
                    kind: Some(DocumentHighlightKind::TEXT),
                }
            })
            .collect()
    }
}
