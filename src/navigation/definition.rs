use std::sync::Arc;

use php_ast::Stmt;
use tower_lsp_server::ls_types::{Location, Position, Range, Uri};

use super::walk::collect_var_refs_in_scope;
use crate::document::ast::{ParsedDoc, SourceView};
use crate::text::word_at_position;
use crate::types::resolve::{Container, Declaration, resolve_declaration};

/// Find the definition of the symbol under `position`.
/// Searches the current document first, then `other_docs` for cross-file resolution.
pub fn goto_definition(
    uri: &Uri,
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[(Uri, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at_position(source, position)?;

    // For $variable, find the first occurrence in scope (= the definition/assignment).
    let sv = doc.view();
    if word.starts_with('$') {
        let bare = word.trim_start_matches('$');
        let byte_off = sv.byte_of_position(position) as usize;
        let mut spans = Vec::new();
        collect_var_refs_in_scope(&doc.program().stmts, bare, byte_off, &mut spans);
        if let Some((span, _)) = spans.into_iter().min_by_key(|(s, _)| s.start) {
            // Promoted property parameters include a visibility keyword before the
            // type and name (`private Database $db`); keep the full span so the cursor
            // lands on the complete declaration. Regular typed params (`int $x`) have
            // span.start at the type — narrow to just $var_name instead.
            let src_at_start = source.get(span.start as usize..).unwrap_or("");
            let is_promoted = src_at_start.starts_with("private ")
                || src_at_start.starts_with("public ")
                || src_at_start.starts_with("protected ")
                || src_at_start.starts_with("readonly ");
            let range = if is_promoted {
                Range {
                    start: sv.position_of(span.start),
                    end: sv.position_of(span.end),
                }
            } else {
                let name_with_sigil = format!("${bare}");
                let precise_start =
                    crate::document::ast::str_offset_in_range(source, span, &name_with_sigil)
                        .unwrap_or(span.start);
                Range {
                    start: sv.position_of(precise_start),
                    end: sv.position_of(precise_start + name_with_sigil.len() as u32),
                }
            };
            return Some(Location {
                uri: uri.clone(),
                range,
            });
        }
    }

    if let Some(range) = resolve_definition_range(sv, &doc.program().stmts, &word) {
        return Some(Location {
            uri: uri.clone(),
            range,
        });
    }

    for (other_uri, other_doc) in other_docs {
        let other_sv = other_doc.view();
        if let Some(range) = resolve_definition_range(other_sv, &other_doc.program().stmts, &word) {
            return Some(Location {
                uri: other_uri.clone(),
                range,
            });
        }
    }

    None
}

/// Like [`goto_definition`], restricted to function declarations in `doc`.
pub fn goto_function_definition(
    uri: &Uri,
    source: &str,
    doc: &ParsedDoc,
    position: Position,
) -> Option<Location> {
    let word = word_at_position(source, position)?;
    let decl = resolve_declaration(&doc.program().stmts, &word, &|d| {
        matches!(d, Declaration::Function { .. })
    })?;
    Some(Location {
        uri: uri.clone(),
        range: definition_name_range(doc.view(), &decl),
    })
}

/// Search an AST for a declaration named `name`, returning its selection range.
/// Used by the PSR-4 fallback in the backend after resolving a class to a file.
pub fn find_declaration_range(_source: &str, doc: &ParsedDoc, name: &str) -> Option<Range> {
    let sv = doc.view();
    resolve_definition_range(sv, &doc.program().stmts, name)
}

/// Resolve `word` to a declaration in `stmts` and return its precise name range.
fn resolve_definition_range(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    word: &str,
) -> Option<Range> {
    // Definition resolves every declaration kind *except* enum constants
    // (which the original walker never matched).
    let decl = resolve_declaration(stmts, word, &|d| {
        !matches!(
            d,
            Declaration::ClassConst {
                container: Container::Enum,
                ..
            }
        )
    })?;
    Some(definition_name_range(sv, &decl))
}

fn definition_name_range(sv: SourceView<'_>, decl: &Declaration<'_>) -> Range {
    sv.name_range_in_span(decl.name(), decl.span())
}
