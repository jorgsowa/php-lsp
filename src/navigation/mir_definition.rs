use std::sync::Arc;

use mir_analyzer::Name;
use tower_lsp_server::ls_types::{Location, Position, Range, Uri};

use crate::document::document_store::{ContentModified, DocumentStore};

/// Declaration of the symbol under `offset`, via mir's `name_at` + `definition_of_cached`.
pub fn mir_definition(
    docs: &DocumentStore,
    uri: &Uri,
    offset: u32,
) -> Result<Option<Location>, ContentModified> {
    let file: Arc<str> = Arc::from(uri.as_str());
    let resolved = docs.with_snapshot(
        |session| session.prepare_for_query(Some(&file)),
        |snap| match snap.name_at(&file, offset)? {
            Some(name) => Ok(snap
                .declaration_name_range_cached(&name)?
                .map(|site| (name, site))),
            None => Ok(None),
        },
    )?;
    Ok(resolved.and_then(|(name, (file, range))| to_lsp_location(docs, &name, &file, range)))
}

/// Declaration locations of the classes named by `fqns`; unknown names are skipped.
pub fn mir_class_locations(
    docs: &DocumentStore,
    fqns: &[String],
) -> Result<Vec<Location>, ContentModified> {
    if fqns.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<Name> = fqns
        .iter()
        .map(|f| Name::Class(Arc::from(f.as_str())))
        .collect();
    let resolved = docs.with_snapshot(
        |_| {},
        |snap| {
            let mut found = Vec::new();
            for name in &names {
                if let Some(site) = snap.declaration_name_range_cached(name)? {
                    found.push((name.clone(), site));
                }
            }
            Ok(found)
        },
    )?;
    let mut locations: Vec<Location> = resolved
        .iter()
        .filter_map(|(name, (file, range))| to_lsp_location(docs, name, file, *range))
        .collect();
    locations.dedup();
    Ok(locations)
}

fn to_lsp_location(
    docs: &DocumentStore,
    name: &Name,
    file: &str,
    range: mir_analyzer::Range,
) -> Option<Location> {
    let target: Uri = file.parse().ok()?;
    let source = docs
        .get_doc_salsa(&target)
        .map(|doc| doc.source().to_owned())
        .unwrap_or_default();
    let mut range = mir_range_to_lsp(&source, range);
    if matches!(name, Name::Method { .. }) {
        range = declared_method_name_range(&source, range).unwrap_or(range);
    }
    Some(Location { range, uri: target })
}

/// A trait alias resolves to the aliased method's whole declaration span; narrow it to the
/// declared name token. `None` when the range is already a single identifier.
fn declared_method_name_range(source: &str, range: Range) -> Option<Range> {
    let line_text = source.split('\n').nth(range.start.line as usize)?;
    let units: Vec<u16> = line_text.encode_utf16().collect();
    let start = range.start.character as usize;
    let end = if range.end.line == range.start.line {
        range.end.character as usize
    } else {
        units.len()
    };
    let window = String::from_utf16(units.get(start..end)?).ok()?;
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    if window.chars().all(is_ident) {
        return None;
    }
    let after = window.find("function")? + "function".len();
    let rest = window[after..].trim_start_matches(|c: char| c.is_whitespace() || c == '&');
    let ident_len = rest.find(|c: char| !is_ident(c)).unwrap_or(rest.len());
    if ident_len == 0 {
        return None;
    }
    let ident_start = window.len() - rest.len();
    let start_char = (start + window[..ident_start].encode_utf16().count()) as u32;
    let end_char = start_char + rest[..ident_len].encode_utf16().count() as u32;
    Some(Range::new(
        Position::new(range.start.line, start_char),
        Position::new(range.start.line, end_char),
    ))
}

/// Calls inside the function or method declared at `offset`, as
/// `(callee declaration name token, call-site range)` in source order.
/// Callees mir cannot locate are skipped.
pub fn mir_outgoing_callees(
    docs: &DocumentStore,
    uri: &Uri,
    offset: u32,
) -> Result<Vec<(Location, Range)>, ContentModified> {
    let file: Arc<str> = Arc::from(uri.as_str());
    let resolved = docs.with_snapshot(
        |session| session.prepare_for_query(Some(&file)),
        |snap| {
            let calls = snap.outgoing_calls(&file, offset)?;
            let mut out = Vec::with_capacity(calls.len());
            for (name, range) in calls {
                if let Some((file, decl)) = snap.declaration_name_range_cached(&name)? {
                    out.push((name, file, decl, range));
                }
            }
            Ok(out)
        },
    )?;
    let call_source = docs.get_doc_salsa(uri);
    Ok(resolved
        .into_iter()
        .filter_map(|(name, file, decl, range)| {
            let target = to_lsp_location(docs, &name, &file, decl)?;
            let source = call_source.as_ref()?.source();
            Some((target, mir_range_to_lsp(source, range)))
        })
        .collect())
}

/// mir ranges are 1-based lines with code-point columns.
fn mir_range_to_lsp(source: &str, range: mir_analyzer::Range) -> Range {
    let convert = |p: mir_analyzer::Position| {
        let line = p.line.saturating_sub(1);
        let text = source.split('\n').nth(line as usize).unwrap_or("");
        let character = text
            .chars()
            .take(p.column as usize)
            .map(char::len_utf16)
            .sum::<usize>() as u32;
        Position::new(line, character)
    };
    Range::new(convert(range.start), convert(range.end))
}
