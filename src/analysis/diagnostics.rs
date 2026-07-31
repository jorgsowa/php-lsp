use std::sync::Arc;

use php_rs_parser::diagnostics::ParseError;
use tower_lsp_server::ls_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

use crate::document::ast::{ParsedDoc, SourceView};

pub const PHP_LSP_SOURCE: &str = "php-lsp";

/// Code for parse-error diagnostics; not "ParseError" since `mir_issues::IssueKind::ParseError` already claims that string.
const PARSE_ERROR_CODE: &str = "SyntaxError";

/// Renders embedded `Span`s as `line:col` instead of raw `Debug` byte offsets.
fn parse_error_message(e: &ParseError, sv: SourceView<'_>) -> String {
    match e {
        ParseError::UnclosedDelimiter {
            delimiter,
            opened_at,
            ..
        } => {
            let pos = sv.position_of(opened_at.start);
            format!(
                "unclosed {delimiter} opened at {}:{}",
                pos.line, pos.character
            )
        }
        _ => e.to_string(),
    }
}

/// Parse `source` without converting parse errors into LSP `Diagnostic`s.
///
/// Hot-path callers (workspace scan, the salsa `parsed_doc` query) discard
/// diagnostics — using this variant skips an O(errors) Vec allocation per
/// file. Callers that actually publish diagnostics call [`parse_document`]
/// instead.
pub fn parse_document_no_diags(source: &str) -> ParsedDoc {
    ParsedDoc::parse(Arc::from(source))
}

/// Build LSP diagnostics from an already-parsed document. Separated from
/// [`parse_document_no_diags`] so the workspace-scan path can skip the
/// allocation entirely.
pub fn diagnostics_from_doc(doc: &ParsedDoc) -> Vec<Diagnostic> {
    let sv = doc.view();
    doc.errors
        .iter()
        .map(|e| {
            let span = e.span();
            let start = sv.position_of(span.start);
            let end = if span.end > span.start {
                sv.position_of(span.end)
            } else {
                // Zero-width span: advance by the UTF-16 width of the character
                // at the error position so the range is never a mid-surrogate
                // slice (characters outside the BMP take 2 UTF-16 code units).
                let ch_width = sv.source()[span.start as usize..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf16() as u32)
                    .unwrap_or(1);
                Position {
                    line: start.line,
                    character: start.character + ch_width,
                }
            };
            Diagnostic {
                range: Range { start, end },
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some(PHP_LSP_SOURCE.to_string()),
                code: Some(NumberOrString::String(PARSE_ERROR_CODE.to_string())),
                message: parse_error_message(e, sv),
                ..Default::default()
            }
        })
        .collect()
}

/// Merge per-file diagnostic categories into one ordered Vec.
///
/// Consistent order: parse errors → semantic issues.
/// All call sites that publish diagnostics for a single file use this function
/// so the ordering is uniform across `did_open`, `did_change`, `document_diagnostic`,
/// `workspace_diagnostic`, and the dependent-republish path.
pub fn merge_file_diagnostics(
    parse: Vec<Diagnostic>,
    semantic: Vec<Diagnostic>,
) -> Vec<Diagnostic> {
    let mut all = parse;
    all.extend(semantic);
    all
}

/// Parse `source` and return the (owned) `ParsedDoc` plus any parse diagnostics.
pub fn parse_document(source: &str) -> (ParsedDoc, Vec<Diagnostic>) {
    let doc = parse_document_no_diags(source);
    let diagnostics = diagnostics_from_doc(&doc);
    (doc, diagnostics)
}
