/// Core AST infrastructure: arena-backed `ParsedDoc`, span utilities, and TypeHint formatting.
use php_ast::{Program, Span, TypeHint, TypeHintKind};
use tower_lsp::lsp_types::{Position, Range};

// ── ParsedDoc ─────────────────────────────────────────────────────────────────

/// Owns a parsed PHP document: the bumpalo arena, source snapshot, and Program.
///
/// SAFETY invariants:
/// - `program` is dropped before `_arena` and `_source` (field declaration order).
/// - Both `_arena` and `_source` are `Box`-allocated; their heap addresses are
///   stable and never move.
/// - The `'static` lifetimes in `Box<Program<'static, 'static>>` are erased
///   versions of the true lifetimes `'_arena` and `'_source`. The public
///   `program()` accessor re-attaches them to `&self`, preventing any reference
///   from escaping beyond the lifetime of the `ParsedDoc`.
pub struct ParsedDoc {
    // Drop order is declaration order in Rust — program drops first.
    program: Box<Program<'static, 'static>>,
    pub errors: Vec<php_rs_parser::diagnostics::ParseError>,
    _arena: Box<bumpalo::Bump>,
    _source: Box<String>,
}

// SAFETY: Program nodes contain only data; no thread-local state.
unsafe impl Send for ParsedDoc {}
unsafe impl Sync for ParsedDoc {}

impl ParsedDoc {
    pub fn parse(source: String) -> Self {
        let source_box = Box::new(source);
        let arena_box = Box::new(bumpalo::Bump::new());

        // SAFETY: Both boxes are on the heap; moving a Box<T> moves the pointer,
        // not the heap data. These references therefore remain valid for as long
        // as the boxes (and hence `self`) are alive.
        let src_ref: &'static str = unsafe {
            std::mem::transmute::<&str, &'static str>(source_box.as_str())
        };
        let arena_ref: &'static bumpalo::Bump = unsafe {
            std::mem::transmute::<&bumpalo::Bump, &'static bumpalo::Bump>(arena_box.as_ref())
        };

        let result = php_rs_parser::parse(arena_ref, src_ref);

        ParsedDoc {
            program: Box::new(result.program),
            errors: result.errors,
            _arena: arena_box,
            _source: source_box,
        }
    }

    /// Borrow the program with lifetimes bounded by `&self`.
    ///
    /// SAFETY: covariance of `Program<'arena, 'src>` in both parameters lets
    /// `&Program<'static, 'static>` shorten to `&Program<'_, '_>`.
    #[inline]
    pub fn program(&self) -> &Program<'_, '_> {
        &*self.program
    }

    /// Borrow the source text used when parsing.
    #[inline]
    pub fn source(&self) -> &str {
        &*self._source
    }
}

impl Default for ParsedDoc {
    fn default() -> Self {
        ParsedDoc::parse(String::new())
    }
}

// ── Span / position utilities ─────────────────────────────────────────────────

/// Convert a byte offset into `source` to an LSP `Position` (0-based line/char).
pub fn offset_to_position(source: &str, offset: u32) -> Position {
    let offset = (offset as usize).min(source.len());
    let prefix = &source[..offset];
    let line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
    let last_nl = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = (offset - last_nl) as u32;
    Position { line, character }
}

/// Convert a `Span` (byte-offset pair) to an LSP `Range`.
pub fn span_to_range(source: &str, span: Span) -> Range {
    Range {
        start: offset_to_position(source, span.start),
        end: offset_to_position(source, span.end),
    }
}

/// Return the byte offset of `substr` within `source`.
///
/// Uses pointer arithmetic when `substr` is a true sub-slice of `source`
/// (i.e. arena-allocated names pointing into the same backing string).
/// Falls back to a content search when the pointers differ — this handles
/// tests and callers that pass a differently-allocated copy of the source.
pub fn str_offset(source: &str, substr: &str) -> u32 {
    let src_ptr = source.as_ptr() as usize;
    let sub_ptr = substr.as_ptr() as usize;
    if sub_ptr >= src_ptr && sub_ptr + substr.len() <= src_ptr + source.len() {
        return (sub_ptr - src_ptr) as u32;
    }
    // Fallback: locate by content (same text, different allocation).
    source.find(substr).unwrap_or(0) as u32
}

/// Build an LSP `Range` for a name that is a sub-slice of `source`.
pub fn name_range(source: &str, name: &str) -> Range {
    let start = str_offset(source, name);
    Range {
        start: offset_to_position(source, start),
        end: offset_to_position(source, start + name.len() as u32),
    }
}

// ── TypeHint formatting ────────────────────────────────────────────────────────

/// Format a `TypeHint` as a PHP type string, e.g. `?int`, `string|null`.
pub fn format_type_hint(hint: &TypeHint<'_, '_>) -> String {
    fmt_kind(&hint.kind)
}

fn fmt_kind(kind: &TypeHintKind<'_, '_>) -> String {
    match kind {
        TypeHintKind::Named(name) => name.to_string_repr().to_string(),
        TypeHintKind::Keyword(builtin, _) => builtin.as_str().to_string(),
        TypeHintKind::Nullable(inner) => format!("?{}", format_type_hint(inner)),
        TypeHintKind::Union(types) => types.iter().map(format_type_hint).collect::<Vec<_>>().join("|"),
        TypeHintKind::Intersection(types) => types.iter().map(format_type_hint).collect::<Vec<_>>().join("&"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_source() {
        let doc = ParsedDoc::parse("<?php".to_string());
        assert!(doc.errors.is_empty());
        assert!(doc.program().stmts.is_empty());
    }

    #[test]
    fn parses_function() {
        let doc = ParsedDoc::parse("<?php\nfunction foo() {}".to_string());
        assert_eq!(doc.program().stmts.len(), 1);
    }

    #[test]
    fn offset_to_position_first_line() {
        assert_eq!(
            offset_to_position("<?php\nfoo", 0),
            Position { line: 0, character: 0 }
        );
    }

    #[test]
    fn offset_to_position_second_line() {
        // "<?php\n" — offset 6 is start of line 1
        assert_eq!(
            offset_to_position("<?php\nfoo", 6),
            Position { line: 1, character: 0 }
        );
    }

    #[test]
    fn str_offset_finds_substr() {
        let src = "<?php\nfunction foo() {}";
        let name = &src[15..18]; // "foo"
        assert_eq!(str_offset(src, name), 15);
    }

    #[test]
    fn str_offset_content_fallback_for_different_allocation() {
        // "foo" is a separately owned String (not a sub-slice of the source),
        // so pointer arithmetic fails. The fallback finds it by content.
        let owned = "foo".to_string();
        assert_eq!(str_offset("<?php foo", &owned), 6);
    }

    #[test]
    fn str_offset_unrelated_content_returns_zero() {
        let owned = "bar".to_string();
        assert_eq!(str_offset("<?php foo", &owned), 0);
    }
}
