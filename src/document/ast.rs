//! Core AST infrastructure: arena-backed `ParsedDoc`, span utilities, and TypeHint formatting.
//!
//! Convention: a borrowed [`SourceView`] is idiomatically bound to `sv` across
//! the crate (offset↔position math). A `ParsedDoc` value is `doc`. See the
//! crate-root glossary in `lib.rs`.
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

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
/// - `_source: Arc<str>` keeps the source text alive at a stable heap address.
///   The transmuted `&'static str` in the parser arena remains valid for the
///   lifetime of `ParsedDoc` because `_source` is dropped last (after `program`
///   is manually dropped and after `_arena` is recycled).
/// - The `'static` lifetimes in `ManuallyDrop<Box<Program<'static, 'static>>>`
///   are erased versions of the true lifetimes `'_arena` and `'_source`. The
///   public `program()` accessor re-attaches them to `&self`, preventing any
///   reference from escaping beyond the lifetime of the `ParsedDoc`.
pub struct ParsedDoc {
    program: ManuallyDrop<Box<Program<'static, 'static>>>,
    pub errors: Vec<php_rs_parser::diagnostics::ParseError>,
    _source: Arc<str>,
    line_starts: Vec<u32>,
    _arena: ArenaGuard,
    /// Memoized `use`-statement local-name → FQN map. Computed on first
    /// access via `file_imports()`; a fresh `ParsedDoc` per edit makes this a
    /// once-per-revision cost instead of once-per-caller (goto-definition,
    /// references, rename, and hover's cross-file fallback all want it).
    imports_cache: OnceLock<Arc<HashMap<String, String>>>,
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
    /// Parse PHP source text. Accepts `String`, `&str`, or `Arc<str>` — when
    /// the caller already holds an `Arc<str>` (e.g. the salsa `parsed_doc`
    /// query), no heap allocation occurs (refcount bump only).
    pub fn parse(source: impl Into<Arc<str>>) -> Self {
        let source: Arc<str> = source.into();
        // Take a pre-warmed arena from the pool (or allocate a fresh one).
        let arena_box = BUMP_POOL.take();

        // SAFETY: `Arc<str>` data lives at a stable heap address for as long as
        // the Arc is alive. `_source` keeps the Arc alive for `ParsedDoc`'s
        // lifetime, so the transmuted `&'static str` remains valid. The arena
        // box is similarly stable — moving a `Box` moves the pointer, not the
        // heap data.
        let src_ref: &'static str = unsafe { std::mem::transmute::<&str, &'static str>(&*source) };
        let arena_ref: &'static bumpalo::Bump = unsafe {
            std::mem::transmute::<&bumpalo::Bump, &'static bumpalo::Bump>(arena_box.as_ref())
        };

        let result = php_rs_parser::parse_arena(arena_ref, src_ref);

        let line_starts = build_line_starts(src_ref);

        ParsedDoc {
            program: ManuallyDrop::new(Box::new(result.program)),
            errors: result.errors,
            _source: source,
            line_starts,
            _arena: ArenaGuard(Some(arena_box)),
            imports_cache: OnceLock::new(),
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

    /// Clone the `Arc<str>` backing the source text (refcount bump only).
    ///
    /// Callers that need to store the same source text in a salsa input
    /// should use this to get the identical pointer — enabling cheap
    /// `Arc::ptr_eq` validation in the `parsed_cache`.
    #[inline]
    pub fn source_arc(&self) -> Arc<str> {
        self._source.clone()
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

    /// `use`-statement local-name → FQN map, memoized on first call for this
    /// `ParsedDoc` (see `imports_cache`). Callers only ever read the map, so
    /// the returned `Arc` is a refcount bump on every call after the first.
    pub fn file_imports(&self) -> Arc<HashMap<String, String>> {
        self.imports_cache
            .get_or_init(|| Arc::new(crate::navigation::references::collect_file_imports(self)))
            .clone()
    }
}

impl Default for ParsedDoc {
    fn default() -> Self {
        ParsedDoc::parse("")
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

    #[inline]
    pub fn line_starts(self) -> &'a [u32] {
        self.line_starts
    }

    /// O(log lines) line number for a byte offset. Cheaper than
    /// `position_of(..).line` because it skips the UTF-16 column scan.
    #[inline]
    pub fn line_of(self, offset: u32) -> u32 {
        match self.line_starts.partition_point(|&s| s <= offset) {
            0 => 0,
            i => (i - 1) as u32,
        }
    }

    /// O(log lines) UTF-16 `Position` -> byte offset. Uses the precomputed
    /// `line_starts` table to jump to the target line and only scans that
    /// line's characters. Much faster than a linear scan over the whole
    /// source when the cursor is late in the file.
    pub fn byte_of_position(self, pos: Position) -> u32 {
        let line_idx = pos.line as usize;
        let line_start = self.line_starts.get(line_idx).copied().unwrap_or(0) as usize;
        let line_end = self
            .line_starts
            .get(line_idx + 1)
            .map(|&s| (s as usize).saturating_sub(1))
            .unwrap_or(self.source.len());
        let raw = &self.source[line_start..line_end.min(self.source.len())];
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let mut col_utf16: u32 = 0;
        let mut byte_in_line: usize = 0;
        for ch in line.chars() {
            if col_utf16 >= pos.character {
                break;
            }
            col_utf16 += ch.len_utf16() as u32;
            byte_in_line += ch.len_utf8();
        }
        (line_start + byte_in_line) as u32
    }

    pub fn range_of(self, span: Span) -> Range {
        Range {
            start: self.position_of(span.start),
            end: self.position_of(span.end),
        }
    }

    pub fn name_range(self, name: &str) -> Range {
        let start = str_offset(self.source, name).unwrap_or(0);
        Range {
            start: self.position_of(start),
            end: self.position_of(start + name.len() as u32),
        }
    }

    /// Like [`name_range`], but searches for `name` *within* `span` instead
    /// of the whole source. Needed when the same name appears in earlier
    /// docblock comments / other declarations — a global search would
    /// otherwise point at the first textual occurrence, not the AST node
    /// the caller actually meant.
    ///
    /// When `name` cannot be located anywhere (e.g. a synthetic `Ident::ERROR`
    /// placeholder such as `"<error>"`, which never occurs in real source),
    /// falls back to an empty range at `span.start` — guaranteed to be
    /// contained in the caller's `span`, unlike byte offset 0.
    pub fn name_range_in_span(self, name: &str, span: php_ast::Span) -> Range {
        let s = span.start as usize;
        let e = (span.end as usize).min(self.source.len());
        let found = self
            .source
            .get(s..e)
            .and_then(|slice| slice.find(name))
            .map(|off| span.start + off as u32)
            .or_else(|| str_offset(self.source, name));
        match found {
            Some(start) => Range {
                start: self.position_of(start),
                end: self.position_of(start + name.len() as u32),
            },
            None => {
                let pos = self.position_of(span.start);
                Range {
                    start: pos,
                    end: pos,
                }
            }
        }
    }

    /// Like [`name_range_in_span`], but excludes any leading `#[...]` attribute
    /// groups from the search span first.
    ///
    /// php-rs-parser captures a member's span starting at its first attribute
    /// token, not at the declaration keyword/name — so searching the raw span
    /// can match `name` as a substring inside an attribute argument (e.g. a
    /// method named `index` matching `'blog_index'` in `#[Route(name: 'blog_index')]`)
    /// instead of the actual identifier.
    pub fn name_range_after_attrs(
        self,
        name: &str,
        attributes: &[php_ast::Attribute<'_, '_>],
        span: php_ast::Span,
    ) -> Range {
        let start = attributes.last().map(|a| a.span.end).unwrap_or(span.start);
        self.name_range_in_span(name, php_ast::Span::new(start, span.end))
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

/// Return the byte offset of `substr` within `source`, or None if not found.
///
/// Uses pointer arithmetic when `substr` is a true sub-slice of `source`
/// (i.e. arena-allocated names pointing into the same backing string).
/// Falls back to a content search when the pointers differ — this handles
/// tests and callers that pass a differently-allocated copy of the source.
///
/// When falling back, prefers matches that are preceded and followed by
/// non-identifier characters (word boundaries), to avoid matching name parts
/// (e.g., finding "B" in "Base" when searching for a class named "B").
pub fn str_offset(source: &str, substr: &str) -> Option<u32> {
    let src_ptr = source.as_ptr() as usize;
    let sub_ptr = substr.as_ptr() as usize;
    if sub_ptr >= src_ptr && sub_ptr + substr.len() <= src_ptr + source.len() {
        return Some((sub_ptr - src_ptr) as u32);
    }
    // Fallback: locate by content with word-boundary preference.
    // When pointer matching fails, prefer matches where substr is surrounded by
    // non-identifier characters to avoid matching partial names (e.g., "B" in "Base").
    let mut search_pos = 0;
    while let Some(offset) = source[search_pos..].find(substr) {
        let abs_offset = search_pos + offset;
        let is_start_boundary = abs_offset == 0
            || !source[..abs_offset]
                .chars()
                .last()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);
        let end_pos = abs_offset + substr.len();
        let is_end_boundary = end_pos >= source.len()
            || !source[end_pos..]
                .chars()
                .next()
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false);

        if is_start_boundary && is_end_boundary {
            return Some(abs_offset as u32);
        }

        search_pos = abs_offset + 1;
    }
    None
}

/// Build an LSP `Range` for a name that is a sub-slice of `source`, or None if not found.
pub fn name_range(source: &str, line_starts: &[u32], name: &str) -> Option<Range> {
    let start = str_offset(source, name)?;
    Some(Range {
        start: offset_to_position(source, line_starts, start),
        end: offset_to_position(source, line_starts, start + name.len() as u32),
    })
}

/// Find a name within a specific byte range of the source, with word-boundary matching.
/// Returns the absolute byte offset if found, None otherwise.
pub fn str_offset_in_range(source: &str, span: Span, name: &str) -> Option<u32> {
    let span_start = span.start as usize;
    let span_end = span.end as usize;
    if span_end > source.len() {
        return None;
    }
    let span_text = &source[span_start..span_end];
    let offset = str_offset(span_text, name)?;
    Some(span_start as u32 + offset)
}

/// The `(alias-or-short-name, FQN)` pair for one `use` import item: the
/// import's local name (an explicit alias when present, otherwise the last
/// segment of the imported FQN) paired with its fully-qualified name.
/// Shared by `navigation::references`'s import-map builder and
/// `index::file_index`'s persistent-index extraction, which otherwise
/// duplicated this exact per-item computation.
pub fn use_item_alias_and_fqn(item: &php_ast::UseItem<'_, '_>) -> (String, String) {
    let fqn = item.name.to_string_repr().into_owned();
    let alias = item
        .alias
        .map(|a| a.to_string())
        .unwrap_or_else(|| crate::text::fqn_short_name(&fqn).to_string());
    (alias, fqn)
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
        assert_eq!(str_offset(src, name), Some(15));
    }

    #[test]
    fn str_offset_content_fallback_for_different_allocation() {
        // "foo" is a separately owned String (not a sub-slice of the source),
        // so pointer arithmetic fails. The fallback finds it by content.
        let owned = "foo".to_string();
        assert_eq!(str_offset("<?php foo", &owned), Some(6));
    }

    #[test]
    fn str_offset_unrelated_content_returns_none() {
        let owned = "bar".to_string();
        assert_eq!(str_offset("<?php foo", &owned), None);
    }

    /// `file_imports()` must walk the AST at most once per `ParsedDoc` —
    /// repeat calls (goto-definition, references, rename, and hover's
    /// cross-file fallback all want the map for the same open file) should
    /// hit the memoized `Arc`, not re-run `collect_file_imports`.
    #[test]
    fn file_imports_is_memoized_across_calls() {
        let doc = ParsedDoc::parse("<?php\nuse App\\Foo;\nuse App\\Bar as B;\n".to_string());

        let first = doc.file_imports();
        assert_eq!(first.get("Foo").map(String::as_str), Some("App\\Foo"));
        assert_eq!(first.get("B").map(String::as_str), Some("App\\Bar"));

        let second = doc.file_imports();
        assert!(
            Arc::ptr_eq(&first, &second),
            "repeat calls must share the memoized map, not recompute it"
        );
    }
}
