//! PHPDoc bareword gate: token positions inside a `/** ... */` docblock that
//! can never resolve to a real declaration — the tag name itself (`param` in
//! `@param`, or any custom/vendor tag like `@psalm-something`), a
//! `@template` parameter name, or the `$varName` half of
//! `@param`/`@var`/`@property*`/`@method` tag bodies.
//!
//! Sibling to `is_bare_keyword_at` (`crate::lang::keywords`): both exist so
//! the bareword fallback used by goto-definition/references/hover/rename
//! never treats a documentation-only token as a searchable symbol. Reuses
//! `php_rs_parser::phpdoc::parse` (the same tag parser `crate::lang::docblock`
//! already delegates to) for tag/span structure instead of re-parsing it by
//! hand — the tag-name half needs no list at all this way: any spelling,
//! built-in or custom, is gated identically.
//!
//! Deliberately NOT gated: the *type* half of these tags (`Foo` in
//! `@param Foo $x`, `@see Foo`) — those already resolve correctly today via
//! the ordinary bareword fallback and must keep doing so.

use php_rs_parser::phpdoc::{self, PhpDocText};
use tower_lsp_server::ls_types::Position;

use crate::text::position_to_byte_offset;

/// Byte range of the `/** ... */` docblock comment containing `offset`, if
/// any. Purely textual (mirrors `docblock_before` in `crate::lang::docblock`):
/// doesn't distinguish a real doc-comment from a `/**`/`*/` pair embedded in
/// a string literal, same class of imprecision already accepted there.
fn docblock_span_containing(source: &str, offset: usize) -> Option<std::ops::Range<usize>> {
    let before = source.get(..offset)?;
    let start = before.rfind("/**")?;
    let after_open = source.get(start + 3..)?;
    // If `*/` already appears before `offset`, this comment closed before
    // reaching the cursor -- it doesn't contain it.
    let offset_in_after_open = offset - (start + 3);
    if after_open[..offset_in_after_open.min(after_open.len())].contains("*/") {
        return None;
    }
    let close_rel = after_open.find("*/")?;
    let end = start + 3 + close_rel + 2;
    Some(start..end)
}

/// Whether `word` at `position` is a documentation-only token inside a
/// docblock that can never resolve to a real declaration.
pub(crate) fn is_unresolvable_docblock_token_at(
    source: &str,
    position: Position,
    word: &str,
) -> bool {
    let offset = position_to_byte_offset(source, position);
    let Some(span) = docblock_span_containing(source, offset) else {
        return false;
    };
    let text = &source[span.clone()];
    let rel_offset = (offset - span.start) as u32;

    let doc = phpdoc::parse(text);
    let Some(tag) = doc
        .tags
        .iter()
        .find(|t| t.span.start <= rel_offset && rel_offset < t.span.end)
    else {
        return false;
    };

    // The tag name itself: `@` + tag.name. Cursor anywhere on the name means
    // this is the annotation token, not a resolvable symbol -- true for
    // every tag, built-in or custom, no list needed.
    let name_end = tag.span.start + 1 + tag.name.len() as u32;
    if rel_offset < name_end {
        return true;
    }

    let Some(body) = &tag.body else {
        return false;
    };
    is_unresolvable_body_token(text, &tag.name, body, rel_offset, word)
}

fn is_unresolvable_body_token(
    doc_text: &str,
    tag_name: &str,
    body: &PhpDocText,
    rel_offset: u32,
    word: &str,
) -> bool {
    let start = body.span.start as usize;
    let end = (body.span.end as usize).min(doc_text.len());
    if start > end || start > doc_text.len() || (rel_offset as usize) < start {
        return false;
    }
    let body_text = &doc_text[start..end];
    let rel_in_body = rel_offset as usize - start;

    // Hyphenated pseudo-types like `non-empty-string` are documentation-only
    // compound tokens. `word_at_position` splits them into bareword segments
    // (`non`, `empty`, `string`), any of which can falsely collide with a real
    // declaration elsewhere in the workspace. If the cursor's word token sits
    // inside a larger non-whitespace body token containing `-`, gate it before
    // the bareword fallback searches for that segment by name.
    if matches!(
        (
            token_range_containing(body_text, rel_in_body),
            non_whitespace_token_range_containing(body_text, rel_in_body),
        ),
        (Some(word_range), Some(compound_range))
            if &body_text[word_range.clone()] == word
                && compound_range.start <= word_range.start
                && word_range.end <= compound_range.end
                && body_text[compound_range.clone()].contains('-')
    ) {
        return true;
    }

    match tag_name.to_ascii_lowercase().as_str() {
        // `@template T` / `@template T of Base`: the parameter name is the
        // first whitespace-separated token, never a resolvable symbol.
        //
        // Compares the cursor's own token range against the first token's
        // range (both via `token_range_containing`'s two-sided expansion)
        // rather than a plain `Range::contains` on `rel_in_body` directly --
        // a cursor sitting exactly at the end of a one-character parameter
        // name like `T` (a real, common position: right after typing it)
        // sits one byte past the half-open range and would otherwise miss.
        "template" | "template-covariant" | "template-contravariant" => {
            match (
                first_token_range(body_text),
                token_range_containing(body_text, rel_in_body),
            ) {
                (Some(first), Some(at_cursor)) => first.start == at_cursor.start,
                _ => false,
            }
        }
        // `Type $varName ...`: the `$varName` token documents a parameter/
        // property/variable name, not a real reference -- but `Type` must
        // keep resolving as it does today, so only ever gate `$`-led words.
        "param" | "var" | "property" | "property-read" | "property-write" | "method" => {
            word.starts_with('$')
                && token_range_containing(body_text, rel_in_body)
                    .is_some_and(|r| &body_text[r] == word)
        }
        _ => false,
    }
}

/// Byte range of the first whitespace-delimited token in `text`, if any.
fn first_token_range(text: &str) -> Option<std::ops::Range<usize>> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let tok_start = text.find(|c: char| !c.is_whitespace())?;
    let rest = &text[tok_start..];
    let tok_len = rest.find(|c: char| !is_word(c)).unwrap_or(rest.len());
    Some(tok_start..tok_start + tok_len)
}

/// Byte range of the enclosing non-whitespace token in `text` containing
/// byte offset `at`, if any.
fn non_whitespace_token_range_containing(text: &str, at: usize) -> Option<std::ops::Range<usize>> {
    let at = at.min(text.len());

    let mut start = at;
    while start > 0 {
        let Some(prev) = text[..start].chars().next_back() else {
            break;
        };
        if prev.is_whitespace() {
            break;
        }
        start -= prev.len_utf8();
    }

    let mut end = at;
    while end < text.len() {
        let Some(next) = text[end..].chars().next() else {
            break;
        };
        if next.is_whitespace() {
            break;
        }
        end += next.len_utf8();
    }

    if start == end { None } else { Some(start..end) }
}

/// Byte range of the identifier/variable token in `text` containing byte
/// offset `at`, if any (`$`-prefixed variables included, matching
/// `word_at_position`'s character class for the sigil).
fn token_range_containing(text: &str, at: usize) -> Option<std::ops::Range<usize>> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let at = at.min(text.len());

    let mut start = at;
    while start > 0 {
        let Some(prev) = text[..start].chars().next_back() else {
            break;
        };
        if !is_word(prev) {
            break;
        }
        start -= prev.len_utf8();
    }

    let mut end = at;
    while end < text.len() {
        let Some(next) = text[end..].chars().next() else {
            break;
        };
        if !is_word(next) {
            break;
        }
        end += next.len_utf8();
    }

    if start == end { None } else { Some(start..end) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos_at_byte_offset(source: &str, byte_offset: usize) -> Position {
        let mut line = 0u32;
        let mut line_start = 0usize;
        for (i, b) in source.bytes().enumerate() {
            if i == byte_offset {
                break;
            }
            if b == b'\n' {
                line += 1;
                line_start = i + 1;
            }
        }
        let character = source[line_start..byte_offset]
            .chars()
            .map(|c| c.len_utf16() as u32)
            .sum();
        Position { line, character }
    }

    fn pos(source: &str, needle: &str) -> Position {
        let byte_offset = source.find(needle).expect("needle not found");
        pos_at_byte_offset(source, byte_offset)
    }

    #[test]
    fn bare_tag_name_is_unresolvable() {
        let src = "<?php\n/**\n * @param int $x\n */\nfunction f($x) {}\n";
        let p = pos(src, "param");
        assert!(is_unresolvable_docblock_token_at(src, p, "param"));
    }

    #[test]
    fn custom_tag_name_is_unresolvable_without_a_list() {
        // No enumeration of tag names anywhere -- any spelling gates the
        // same way, including vendor-specific tags no list would cover.
        let src = "<?php\n/**\n * @psalm-immutable\n */\nclass C {}\n";
        let p = pos(src, "psalm-immutable");
        assert!(is_unresolvable_docblock_token_at(src, p, "psalm-immutable"));
    }

    #[test]
    fn template_parameter_name_is_unresolvable() {
        let src = "<?php\n/**\n * @template T of Base\n */\nclass Box {}\n";
        let p = pos(src, "T of");
        assert!(is_unresolvable_docblock_token_at(src, p, "T"));
    }

    #[test]
    fn template_parameter_name_is_unresolvable_at_its_end_boundary() {
        // Cursor sitting exactly at the end of `T` (right after typing it,
        // one byte past the token's half-open range) must still gate --
        // this is the common cursor position after typing a short name.
        let src = "<?php\n/**\n * @template T of Base\n */\nclass Box {}\n";
        let end_of_t = src.find("T of").expect("needle not found") + 1;
        let p = pos_at_byte_offset(src, end_of_t);
        assert!(is_unresolvable_docblock_token_at(src, p, "T"));
    }

    #[test]
    fn template_bound_type_still_resolves() {
        // `Base` is a real type name in `@template T of Base` -- must not be
        // gated, the same way `@see`/`@param Type` type names aren't.
        let src = "<?php\n/**\n * @template T of Base\n */\nclass Box {}\n";
        let p = pos(src, "Base");
        assert!(!is_unresolvable_docblock_token_at(src, p, "Base"));
    }

    #[test]
    fn param_doc_variable_name_is_unresolvable() {
        let src = "<?php\n/**\n * @param string $userName\n */\nfunction f($userName) {}\n";
        let p = pos(src, "$userName");
        assert!(is_unresolvable_docblock_token_at(src, p, "$userName"));
    }

    #[test]
    fn param_type_hint_still_resolves() {
        let src = "<?php\n/**\n * @param Widget $w\n */\nfunction f($w) {}\n";
        let p = pos(src, "Widget");
        assert!(!is_unresolvable_docblock_token_at(src, p, "Widget"));
    }

    #[test]
    fn var_doc_variable_name_is_unresolvable() {
        let src =
            "<?php\nclass C {\n    /**\n     * @var int $count\n     */\n    public $count;\n}\n";
        let p = pos(src, "$count\n");
        assert!(is_unresolvable_docblock_token_at(src, p, "$count"));
    }

    #[test]
    fn property_doc_variable_name_is_unresolvable() {
        let src = "<?php\n/**\n * @property-read string $name\n */\nclass User {}\n";
        let p = pos(src, "$name");
        assert!(is_unresolvable_docblock_token_at(src, p, "$name"));
    }

    #[test]
    fn method_doc_parameter_name_is_unresolvable() {
        let src = "<?php\n/**\n * @method static self make(string $name)\n */\nclass Factory {}\n";
        let p = pos(src, "$name)");
        assert!(is_unresolvable_docblock_token_at(src, p, "$name"));
    }

    #[test]
    fn see_tag_target_still_resolves() {
        let src = "<?php\n/**\n * @see Helper\n */\nfunction f() {}\n";
        let p = pos(src, "Helper");
        assert!(!is_unresolvable_docblock_token_at(src, p, "Helper"));
    }

    #[test]
    fn hyphenated_pseudo_type_segment_is_unresolvable() {
        let src = "<?php\n/**\n * @param non-empty-string $value\n */\nfunction f($value) {}\n";
        let p = pos(src, "non-empty-string");
        assert!(is_unresolvable_docblock_token_at(src, p, "non"));
    }

    #[test]
    fn word_outside_any_docblock_is_not_gated() {
        let src = "<?php\nfunction from() {}\n";
        let p = pos(src, "from");
        assert!(!is_unresolvable_docblock_token_at(src, p, "from"));
    }

    #[test]
    fn word_in_plain_block_comment_is_not_gated() {
        // `/* ... */` (no second `*`) is not a doc-block -- only `/**` is.
        let src = "<?php\n/* @param int $x */\nfunction f($x) {}\n";
        let p = pos(src, "param");
        assert!(!is_unresolvable_docblock_token_at(src, p, "param"));
    }

    #[test]
    fn word_after_closed_docblock_is_not_gated() {
        let src = "<?php\n/** @param int $x */\n$param = 1;\n";
        let p = pos(src, "$param =");
        assert!(!is_unresolvable_docblock_token_at(src, p, "$param"));
    }
}
