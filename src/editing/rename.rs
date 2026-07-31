use std::collections::HashMap;
use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{Expr, ExprKind};
use tower_lsp_server::ls_types::{Position, Range, TextEdit, Uri, WorkspaceEdit};

use crate::document::ast::ParsedDoc;
use crate::lang::is_php_keyword;
use crate::navigation::walk::collect_var_refs_in_scope;
use crate::text::utf16_code_units;

/// Narrow `range` to the last whole-word, case-sensitive occurrence of
/// `word` inside it — or reject the location entirely.
///
/// mir's posting spans cover the full written name — a qualified usage
/// (`\App\Widget`), a `use` import item (`App\Widget as W`), or a property
/// declaration (`$count`) — while a rename must replace only the renamed
/// token: the final path segment, or the alias when that is what's being
/// renamed. `None` means the span doesn't contain the word at all (an
/// aliased usage site, or a name differing only in case — mir resolves
/// per PHP's case-insensitive semantics, but rename edits follow the
/// case-sensitive editor convention) and must not be edited.
pub fn narrow_range_to_word(source: &str, range: Range, word: &str) -> Option<Range> {
    if word.is_empty() || range.start.line != range.end.line {
        return None;
    }
    let line = source.lines().nth(range.start.line as usize)?;
    // UTF-16 column → byte offset within the line.
    let byte_at_col = |col: u32| -> Option<usize> {
        let mut u16s: u32 = 0;
        if col == 0 {
            return Some(0);
        }
        for (i, ch) in line.char_indices() {
            if u16s == col {
                return Some(i);
            }
            u16s += ch.len_utf16() as u32;
        }
        (u16s >= col).then_some(line.len())
    };
    let start_b = byte_at_col(range.start.character)?;
    let end_b = byte_at_col(range.end.character)?;
    let slice = line.get(start_b..end_b)?;
    if slice == word {
        return Some(range);
    }

    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut found: Option<usize> = None;
    let mut search = 0usize;
    while let Some(pos) = slice[search..].find(word) {
        let abs = search + pos;
        let before_ok = abs == 0 || !slice[..abs].chars().next_back().is_some_and(is_word_char);
        let after = abs + word.len();
        let after_ok =
            after >= slice.len() || !slice[after..].chars().next().is_some_and(is_word_char);
        if before_ok && after_ok {
            found = Some(abs);
        }
        search = after.max(abs + 1);
    }
    let off = found?;

    let new_start = range.start.character + utf16_code_units(&slice[..off]);
    Some(Range {
        start: Position {
            line: range.start.line,
            character: new_start,
        },
        end: Position {
            line: range.start.line,
            character: new_start + utf16_code_units(word),
        },
    })
}

/// Sorts `edits` by start position and drops duplicate ranges — the shared
/// finishing step for a `WorkspaceEdit`'s per-file edit list.
pub fn sort_and_dedup_edits(edits: &mut Vec<TextEdit>) {
    edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));
    edits.dedup_by(|a, b| a.range == b.range);
}

/// Returns the range of the word at `position` if it's a renameable symbol.
/// Used for `textDocument/prepareRename`.
pub fn prepare_rename(doc: &ParsedDoc, position: Position) -> Option<Range> {
    use crate::text::word_at_position;
    let source = doc.source();
    let word = word_at_position(source, position)?;
    if word.contains('\\') {
        return None;
    }
    // PHP keywords cannot be renamed when used as keywords — but almost all
    // of them (`match`, `list`, `enum`, `static`, `readonly`, `default`, ...)
    // are valid method names, and any keyword-named method call/declaration
    // is therefore a real renameable symbol, not a keyword use.
    if is_php_keyword(&word) && !is_member_name_position(doc, position) {
        return None;
    }
    // PHP superglobals ($_GET, $_POST, etc.) are part of the language runtime;
    // renaming them breaks code, so we disable the action.
    if is_superglobal(&word) {
        return None;
    }
    let line = source.lines().nth(position.line as usize)?;
    let col = position.character as usize;
    let chars: Vec<char> = line.chars().collect();
    // `is_word` intentionally excludes `$` so the range covers only the bare
    // identifier name (not the sigil). `word_at` may return `$var` with the `$`,
    // so we strip it before computing the range length to avoid an off-by-one.
    let is_word = |c: char| c.is_alphanumeric() || c == '_';

    // Find the character index at or before the cursor position (in UTF-16 code units)
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for (i, ch) in chars.iter().enumerate() {
        // Check if cursor is within this character's UTF-16 span
        let char_width = ch.len_utf16();
        if utf16_col + char_width > col {
            char_idx = i;
            break;
        }
        utf16_col += char_width;
        char_idx = i + 1;
    }

    // Find the start of the word by walking backwards
    let mut left = char_idx;
    while left > 0 && is_word(chars[left - 1]) {
        left -= 1;
    }

    let bare_word = word.trim_start_matches('$');
    let start_utf16: u32 = chars[..left].iter().map(|c| c.len_utf16() as u32).sum();
    let end_utf16: u32 = start_utf16 + utf16_code_units(bare_word);
    Some(Range {
        start: Position {
            line: position.line,
            character: start_utf16,
        },
        end: Position {
            line: position.line,
            character: end_utf16,
        },
    })
}

/// True when the word at `position` sits in a position where PHP allows a
/// keyword as an ordinary identifier: the member-name side of a method call,
/// static method/const access, or property access (`->`, `?->`, `::`), or
/// right after the `function` keyword (a method declaration — `function
/// match(): void {}` only parses inside a class/trait/interface/enum body,
/// since the same name at true top level is a syntax error, so this check
/// needs no extra nesting context).
///
/// Member access is checked via the AST rather than the cursor's own line,
/// so a receiver wrapped across lines (`$obj\n    ->list()`) still resolves
/// — a same-line text check would miss the `->` on the previous line and
/// wrongly block renaming a keyword-named method. Declarations stay a
/// same-line text check: `function` and the method name are always adjacent
/// tokens, and `Ident` carries no span for an AST-based lookup.
fn is_member_name_position(doc: &ParsedDoc, position: Position) -> bool {
    use crate::text::word_range_at;
    let source = doc.source();
    let Some(range) = word_range_at(source, position) else {
        return false;
    };
    let offset = doc.view().byte_of_position(range.start);
    if is_member_access_name(doc, offset) {
        return true;
    }
    let Some(line) = source.lines().nth(range.start.line as usize) else {
        return false;
    };
    let before = utf16_prefix(line, range.start.character).trim_end();
    match before.strip_suffix("function") {
        Some(rest) => rest
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_'),
        None => false,
    }
}

/// True when `offset` lands inside the member-name token of a method call,
/// static method/const access, or property access anywhere in `doc`.
fn is_member_access_name(doc: &ParsedDoc, offset: u32) -> bool {
    struct Finder {
        offset: u32,
        found: bool,
    }

    fn at(span: php_ast::Span, offset: u32) -> bool {
        span.start <= offset && offset < span.end
    }

    impl<'arena, 'src> Visitor<'arena, 'src> for Finder {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if self.found {
                return ControlFlow::Break(());
            }
            let hit = match &expr.kind {
                ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
                    at(m.method.span, self.offset)
                }
                ExprKind::StaticMethodCall(s) => at(s.method.span, self.offset),
                ExprKind::StaticDynMethodCall(s) => at(s.method.span, self.offset),
                ExprKind::PropertyAccess(p) | ExprKind::NullsafePropertyAccess(p) => {
                    at(p.property.span, self.offset)
                }
                ExprKind::StaticPropertyAccess(s) | ExprKind::ClassConstAccess(s) => {
                    at(s.member.span, self.offset)
                }
                _ => false,
            };
            if hit {
                self.found = true;
                return ControlFlow::Break(());
            }
            walk_expr(self, expr)
        }
    }

    let mut finder = Finder {
        offset,
        found: false,
    };
    for stmt in doc.program().stmts.iter() {
        let _ = finder.visit_stmt(stmt);
    }
    finder.found
}

/// The substring of `line` up to UTF-16 column `col`.
fn utf16_prefix(line: &str, col: u32) -> &str {
    let mut u16s = 0u32;
    for (i, ch) in line.char_indices() {
        if u16s >= col {
            return &line[..i];
        }
        u16s += ch.len_utf16() as u32;
    }
    line
}

pub(crate) fn is_superglobal(word: &str) -> bool {
    matches!(
        word,
        "$_GET"
            | "$_POST"
            | "$_REQUEST"
            | "$_FILES"
            | "$_COOKIE"
            | "$_SESSION"
            | "$_SERVER"
            | "$_ENV"
            | "$GLOBALS"
            | "$this"
    )
}

/// Rename a `$variable` (or parameter) within its enclosing function/method scope.
/// Only produces edits within the single document `uri`; variables don't cross files.
pub fn rename_variable(
    var_name: &str,
    new_name: &str,
    uri: &Uri,
    doc: &ParsedDoc,
    position: Position,
) -> WorkspaceEdit {
    let bare = var_name.trim_start_matches('$');
    let new_bare = new_name.trim_start_matches('$');
    let new_text = format!("${new_bare}");

    let stmts = &doc.program().stmts;
    let sv = doc.view();
    let byte_off = sv.byte_of_position(position) as usize;

    let mut spans = Vec::new();
    collect_var_refs_in_scope(stmts, bare, byte_off, &mut spans);

    let mut seen = std::collections::HashSet::new();
    let mut edits: Vec<TextEdit> = spans
        .into_iter()
        .filter_map(|(span, _)| {
            let start = sv.position_of(span.start);
            let end = sv.position_of(span.end);
            seen.insert((start.line, start.character))
                .then_some(TextEdit {
                    range: Range { start, end },
                    new_text: new_text.clone(),
                })
        })
        .collect();
    edits.sort_by_key(|e| (e.range.start.line, e.range.start.character));

    let mut changes = HashMap::new();
    if !edits.is_empty() {
        changes.insert(uri.clone(), edits);
    }

    WorkspaceEdit {
        changes: if changes.is_empty() {
            None
        } else {
            Some(changes)
        },
        ..Default::default()
    }
}
