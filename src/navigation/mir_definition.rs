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
                .definition_of_cached(&name)?
                .ok()
                .map(|loc| (name, loc))),
            None => Ok(None),
        },
    )?;
    Ok(resolved.and_then(|(name, loc)| to_lsp_location(docs, &name, &loc)))
}

fn to_lsp_location(
    docs: &DocumentStore,
    name: &Name,
    loc: &mir_types::Location,
) -> Option<Location> {
    let target: Uri = loc.file.parse().ok()?;
    let span_start = Position::new(loc.line.saturating_sub(1), u32::from(loc.col_start));
    let span_end = Position::new(loc.line_end.saturating_sub(1), u32::from(loc.col_end));
    // No name token in the span (e.g. a docblock `@method`): defer to the AST fallbacks.
    let range = match docs.get_doc_salsa(&target) {
        Some(doc) => name_range(doc.source(), name, span_start, span_end)
            .or_else(|| docblock_method_range(doc.source(), name, span_start.line))?,
        None => Range::new(span_start, span_end),
    };
    Some(Location { uri: target, range })
}

/// Narrows a declaration span to its name token (columns are code points).
fn name_range(
    source: &str,
    name: &Name,
    span_start: Position,
    span_end: Position,
) -> Option<Range> {
    let (needle, skip_keywords): (String, &[&str]) = match name {
        Name::Class(fqn) => (
            short(fqn).to_owned(),
            &["class", "interface", "trait", "enum"],
        ),
        Name::Function(fqn) => (short(fqn).to_owned(), &["function"]),
        Name::Method { name, .. } => (name.to_string(), &["function"]),
        Name::Property { name, .. } => (format!("${name}"), &[]),
        Name::ClassConstant { name, .. } => (name.to_string(), &["const", "case"]),
        Name::GlobalConstant(fqn) => (short(fqn).to_owned(), &["const"]),
    };

    let mut lines = source.split('\n');
    let line_text = lines.nth(span_start.line as usize)?;
    let single_line = span_end.line == span_start.line;
    let start_byte = char_to_byte(line_text, span_start.character as usize);
    let end_byte = if single_line {
        char_to_byte(line_text, span_end.character as usize)
    } else {
        line_text.len()
    };
    let window = line_text.get(start_byte..end_byte)?;

    let mut from = 0;
    if !skip_keywords.is_empty() {
        from = skip_keywords
            .iter()
            .filter_map(|kw| find_word(window, kw, 0).map(|i| i + kw.len()))
            .min()
            .unwrap_or(0);
    }
    let (hit, len) =
        match find_word(window, &needle, from).or_else(|| find_word(window, &needle, 0)) {
            Some(hit) => (hit, needle.len()),
            // A trait alias resolves to the aliased method's declaration.
            None if matches!(name, Name::Method { .. }) => declared_method_name(window)?,
            None => return None,
        };
    // Selection excludes a property's `$` sigil.
    let sigil = usize::from(matches!(name, Name::Property { .. }));
    let abs = start_byte + hit + sigil;
    let start_char = line_text[..abs].encode_utf16().count() as u32;
    let end_char = start_char + line_text[abs..abs + len - sigil].encode_utf16().count() as u32;
    Some(Range::new(
        Position::new(span_start.line, start_char),
        Position::new(span_start.line, end_char),
    ))
}

/// Name token of an `@method` tag in the docblock directly above line `decl_line`.
fn docblock_method_range(source: &str, name: &Name, decl_line: u32) -> Option<Range> {
    let Name::Method { name, .. } = name else {
        return None;
    };
    let lines: Vec<&str> = source.split('\n').collect();
    let mut i = (decl_line as usize).min(lines.len());
    while i > 0 {
        i -= 1;
        let line = lines[i].trim();
        if !(line.starts_with('*') || line.starts_with("/**")) {
            break;
        }
        let Some(tag) = find_word(lines[i], "@method", 0).map(|t| t + "@method".len()) else {
            continue;
        };
        // The method name is the identifier directly before the `(`.
        let Some(paren) = lines[i][tag..].find('(').map(|p| tag + p) else {
            continue;
        };
        let Some(start) = find_word(&lines[i][..paren], name, tag) else {
            continue;
        };
        let start_char = lines[i][..start].encode_utf16().count() as u32;
        let end_char = start_char + name.encode_utf16().count() as u32;
        return Some(Range::new(
            Position::new(i as u32, start_char),
            Position::new(i as u32, end_char),
        ));
    }
    None
}

/// Offset and length of the identifier following the first `function` keyword.
fn declared_method_name(window: &str) -> Option<(usize, usize)> {
    let after = find_word(window, "function", 0)? + "function".len();
    let rest = &window[after..];
    let skipped = rest.len()
        - rest
            .trim_start_matches(|c: char| c.is_whitespace() || c == '&')
            .len();
    let ident = &rest[skipped..];
    let len = ident
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(ident.len());
    (len > 0).then_some((after + skipped, len))
}

fn short(fqn: &str) -> &str {
    fqn.rsplit('\\').next().unwrap_or(fqn)
}

fn char_to_byte(line: &str, chars: usize) -> usize {
    line.char_indices()
        .nth(chars)
        .map_or(line.len(), |(i, _)| i)
}

/// ASCII-case-insensitive whole-identifier search starting at byte `from`.
fn find_word(haystack: &str, word: &str, from: usize) -> Option<usize> {
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let bytes = haystack.as_bytes();
    let wl = word.len();
    let mut i = from;
    while i + wl <= bytes.len() {
        if haystack.is_char_boundary(i)
            && bytes[i..i + wl].eq_ignore_ascii_case(word.as_bytes())
            && (word.starts_with('$') || i == 0 || !is_ident(bytes[i - 1]))
            && (i + wl == bytes.len() || !is_ident(bytes[i + wl]))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}
