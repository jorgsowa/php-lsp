/// Core AST infrastructure: arena-backed `ParsedDoc`, span utilities, and TypeHint formatting.
use std::mem::ManuallyDrop;
use std::sync::{LazyLock, Mutex};

use php_ast::{Program, Span, TypeHint, TypeHintKind};
use tower_lsp::lsp_types::{Position, Range};

// ── BumpPool ──────────────────────────────────────────────────────────────────

const POOL_CAP: usize = 8;

struct BumpPool {
    // Box<Bump> required: arena_box.as_ref() is transmuted to &'static Bump in
    // ParsedDoc::parse(). The Box keeps the Bump at a stable heap address so
    // that reference remains valid after the Box is moved into ArenaGuard.
    #[allow(clippy::vec_box)]
    pool: Mutex<Vec<Box<bumpalo::Bump>>>,
}

impl BumpPool {
    fn take(&self) -> Box<bumpalo::Bump> {
        self.pool
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| Box::new(bumpalo::Bump::new()))
    }

    fn give(&self, mut arena: Box<bumpalo::Bump>) {
        arena.reset();
        let mut p = self.pool.lock().unwrap();
        if p.len() < POOL_CAP {
            p.push(arena);
        }
    }
}

static BUMP_POOL: LazyLock<BumpPool> = LazyLock::new(|| BumpPool {
    pool: Mutex::new(Vec::new()),
});

// ── ArenaGuard ────────────────────────────────────────────────────────────────

/// Returns the arena to the pool on drop.
struct ArenaGuard(Option<Box<bumpalo::Bump>>);

impl Drop for ArenaGuard {
    fn drop(&mut self) {
        if let Some(arena) = self.0.take() {
            BUMP_POOL.give(arena);
        }
    }
}

// ── ParsedDoc ─────────────────────────────────────────────────────────────────

/// Owns a parsed PHP document: the bumpalo arena, source snapshot, and Program.
///
/// SAFETY invariants:
/// - `program` uses `ManuallyDrop` and is explicitly dropped in `Drop::drop()`
///   before any field auto-drop runs. This guarantees arena-allocated nodes are
///   gone before `ArenaGuard` recycles the arena — regardless of field order.
/// - Both `_arena` and `_source` are `Box`-allocated; their heap addresses are
///   stable and never move.
/// - The `'static` lifetimes in `ManuallyDrop<Box<Program<'static, 'static>>>`
///   are erased versions of the true lifetimes `'_arena` and `'_source`. The
///   public `program()` accessor re-attaches them to `&self`, preventing any
///   reference from escaping beyond the lifetime of the `ParsedDoc`.
pub struct ParsedDoc {
    program: ManuallyDrop<Box<Program<'static, 'static>>>,
    pub errors: Vec<php_rs_parser::diagnostics::ParseError>,
    #[allow(clippy::box_collection)]
    _source: Box<String>,
    line_starts: Vec<u32>,
    _arena: ArenaGuard,
}

impl Drop for ParsedDoc {
    fn drop(&mut self) {
        // Drop program explicitly before any field auto-drop runs (including
        // _arena). ManuallyDrop prevents a second drop after this method returns.
        // SAFETY: called exactly once here; no other code drops this field.
        unsafe { ManuallyDrop::drop(&mut self.program) };
    }
}

// SAFETY: Program nodes contain only data; no thread-local state.
unsafe impl Send for ParsedDoc {}
unsafe impl Sync for ParsedDoc {}

impl ParsedDoc {
    pub fn parse(source: String) -> Self {
        let source_box = Box::new(source);
        // Take a pre-warmed arena from the pool (or allocate a fresh one).
        let arena_box = BUMP_POOL.take();

        // SAFETY: Both boxes are on the heap; moving a Box<T> moves the pointer,
        // not the heap data. These references therefore remain valid for as long
        // as the boxes (and hence `self`) are alive.
        let src_ref: &'static str =
            unsafe { std::mem::transmute::<&str, &'static str>(source_box.as_str()) };
        let arena_ref: &'static bumpalo::Bump = unsafe {
            std::mem::transmute::<&bumpalo::Bump, &'static bumpalo::Bump>(arena_box.as_ref())
        };

        let result = php_rs_parser::parse(arena_ref, src_ref);

        let line_starts = build_line_starts(src_ref);

        ParsedDoc {
            program: ManuallyDrop::new(Box::new(result.program)),
            errors: result.errors,
            _source: source_box,
            line_starts,
            _arena: ArenaGuard(Some(arena_box)),
        }
    }

    /// Borrow the program with lifetimes bounded by `&self`.
    ///
    /// SAFETY: covariance of `Program<'arena, 'src>` in both parameters lets
    /// `&Program<'static, 'static>` shorten to `&Program<'_, '_>`.
    #[inline]
    pub fn program(&self) -> &Program<'_, '_> {
        &self.program
    }

    /// Borrow the source text used when parsing.
    #[inline]
    pub fn source(&self) -> &str {
        &self._source
    }

    /// Borrow the precomputed line-start byte offsets.
    /// `line_starts[i]` is the byte offset of the first character on line `i`.
    pub fn line_starts(&self) -> &[u32] {
        &self.line_starts
    }

    /// Bundle source and line index for position lookups.
    pub fn view(&self) -> SourceView<'_> {
        SourceView {
            source: self.source(),
            line_starts: self.line_starts(),
        }
    }
}

impl Default for ParsedDoc {
    fn default() -> Self {
        ParsedDoc::parse(String::new())
    }
}

// ── Span / position utilities ─────────────────────────────────────────────────

/// Build a table of byte offsets for the start of each line.
/// `result[i]` is the byte offset of the first character on line `i`.
fn build_line_starts(source: &str) -> Vec<u32> {
    let mut starts = vec![0u32];
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i as u32 + 1);
        }
    }
    starts
}

/// Bundles source text with its precomputed line-start table.
/// `Copy` so inner functions can pass it by value without lifetime annotation churn.
#[derive(Copy, Clone)]
pub struct SourceView<'a> {
    source: &'a str,
    line_starts: &'a [u32],
}

impl<'a> SourceView<'a> {
    #[inline]
    pub fn source(self) -> &'a str {
        self.source
    }

    pub fn position_of(self, offset: u32) -> Position {
        offset_to_position(self.source, self.line_starts, offset)
    }

    pub fn range_of(self, span: Span) -> Range {
        Range {
            start: self.position_of(span.start),
            end: self.position_of(span.end),
        }
    }

    pub fn name_range(self, name: &str) -> Range {
        let start = str_offset(self.source, name);
        Range {
            start: self.position_of(start),
            end: self.position_of(start + name.len() as u32),
        }
    }
}

/// Convert a byte offset into `source` to an LSP `Position` (0-based line/char).
///
/// Uses a precomputed `line_starts` table for O(log n) binary search.
/// Handles both LF-only and CRLF line endings: a trailing `\r` before `\n` is
/// not counted as a column so that positions are consistent regardless of
/// line-ending style.
pub fn offset_to_position(source: &str, line_starts: &[u32], offset: u32) -> Position {
    let offset_usize = (offset as usize).min(source.len());
    // Binary search: find the last line_start ≤ offset.
    let line = match line_starts.partition_point(|&s| s <= offset) {
        0 => 0u32,
        i => (i - 1) as u32,
    };
    let line_start = line_starts.get(line as usize).copied().unwrap_or(0) as usize;
    let segment = &source[line_start..offset_usize];
    // Strip trailing \r to handle CRLF: don't count \r as a column.
    let segment = segment.strip_suffix('\r').unwrap_or(segment);
    let character = segment.chars().map(|c| c.len_utf16() as u32).sum::<u32>();
    Position { line, character }
}

/// Convert a `Span` (byte-offset pair) to an LSP `Range`.
pub fn span_to_range(source: &str, line_starts: &[u32], span: Span) -> Range {
    Range {
        start: offset_to_position(source, line_starts, span.start),
        end: offset_to_position(source, line_starts, span.end),
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
pub fn name_range(source: &str, line_starts: &[u32], name: &str) -> Range {
    let start = str_offset(source, name);
    Range {
        start: offset_to_position(source, line_starts, start),
        end: offset_to_position(source, line_starts, start + name.len() as u32),
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
        TypeHintKind::Union(types) => types
            .iter()
            .map(format_type_hint)
            .collect::<Vec<_>>()
            .join("|"),
        TypeHintKind::Intersection(types) => types
            .iter()
            .map(format_type_hint)
            .collect::<Vec<_>>()
            .join("&"),
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
        let src = "<?php\nfoo";
        let doc = ParsedDoc::parse(src.to_string());
        assert_eq!(
            offset_to_position(src, doc.line_starts(), 0),
            Position {
                line: 0,
                character: 0
            }
        );
    }

    #[test]
    fn offset_to_position_second_line() {
        // "<?php\n" — offset 6 is start of line 1
        let src = "<?php\nfoo";
        let doc = ParsedDoc::parse(src.to_string());
        assert_eq!(
            offset_to_position(src, doc.line_starts(), 6),
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn offset_to_position_multibyte_utf16() {
        // "é" is U+00E9: 2 UTF-8 bytes, 1 UTF-16 code unit.
        // "😀" is U+1F600: 4 UTF-8 bytes, 2 UTF-16 code units.
        // source: "a😀b" — byte offsets: a=0, 😀=1..5, b=5
        // UTF-16:            a=col 0, 😀=col 1..3, b=col 3
        let src = "a\u{1F600}b";
        let doc = ParsedDoc::parse(src.to_string());
        assert_eq!(
            offset_to_position(src, doc.line_starts(), 5), // byte offset of 'b'
            Position {
                line: 0,
                character: 3
            }  // UTF-16 col 3
        );
    }

    #[test]
    fn offset_to_position_crlf_start_of_line() {
        // CRLF: offset pointing to first char of line 1 must give character=0.
        // "foo\r\nbar": f=0 o=1 o=2 \r=3 \n=4 b=5 a=6 r=7
        let src = "foo\r\nbar";
        let doc = ParsedDoc::parse(src.to_string());
        assert_eq!(
            offset_to_position(src, doc.line_starts(), 5), // 'b'
            Position {
                line: 1,
                character: 0
            }
        );
    }

    #[test]
    fn offset_to_position_crlf_does_not_count_cr_in_column() {
        // Offset pointing to the \r itself must not count it as a column.
        // "foo\r\nbar": the \r is at offset 3, column must be 3 (length of "foo").
        let src = "foo\r\nbar";
        let doc = ParsedDoc::parse(src.to_string());
        assert_eq!(
            offset_to_position(src, doc.line_starts(), 3), // '\r'
            Position {
                line: 0,
                character: 3
            }
        );
    }

    #[test]
    fn offset_to_position_crlf_multiline() {
        // Multiple CRLF lines: columns must not accumulate stray \r counts.
        // "a\r\nb\r\nc": a=0 \r=1 \n=2 b=3 \r=4 \n=5 c=6
        let src = "a\r\nb\r\nc";
        let doc = ParsedDoc::parse(src.to_string());
        assert_eq!(
            offset_to_position(src, doc.line_starts(), 6), // 'c'
            Position {
                line: 2,
                character: 0
            }
        );
        assert_eq!(
            offset_to_position(src, doc.line_starts(), 3), // 'b'
            Position {
                line: 1,
                character: 0
            }
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
