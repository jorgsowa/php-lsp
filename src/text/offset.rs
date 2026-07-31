//! UTF-16 ↔ UTF-8 offset conversions and incremental text-change application.
//!
//! LSP positions count UTF-16 code units; Rust strings are UTF-8. These helpers
//! bridge the two.

use tower_lsp_server::ls_types::{Position, Range};

/// Convert a UTF-16 code unit offset into a UTF-8 byte offset for `s`.
///
/// LSP positions use UTF-16 code units; Rust strings are UTF-8.  This helper
/// walks the string's `char_indices`, accumulating UTF-16 units, and returns
/// the byte index of the character at the given UTF-16 offset.  If the offset
/// is past the end of the string, `s.len()` is returned.
pub(crate) fn utf16_offset_to_byte(s: &str, utf16_offset: usize) -> usize {
    let mut utf16_count = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        if utf16_count >= utf16_offset {
            return byte_idx;
        }
        utf16_count += ch.len_utf16();
    }
    s.len()
}

/// Convert an LSP `Position` (line + UTF-16 character column) into a byte
/// offset in `text`. Out-of-range lines clamp to `text.len()`; out-of-range
/// columns clamp to the end of the line (before its `\n`). Used by
/// incremental text sync.
pub(crate) fn position_to_byte_offset(text: &str, pos: Position) -> usize {
    let mut line_start = 0usize;
    for _ in 0..pos.line {
        match text[line_start..].find('\n') {
            Some(i) => line_start += i + 1,
            None => return text.len(),
        }
    }
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |i| line_start + i);
    line_start + utf16_offset_to_byte(&text[line_start..line_end], pos.character as usize)
}

/// Apply one LSP incremental content change (replace `range` with `new_text`)
/// to `text`. A malformed range whose end precedes its start degrades to an
/// insertion at `start`.
pub(crate) fn apply_content_change(text: &mut String, range: Range, new_text: &str) {
    let start = position_to_byte_offset(text, range.start);
    let end = position_to_byte_offset(text, range.end).max(start);
    text.replace_range(start..end, new_text);
}

/// Convert a UTF-8 byte offset into a UTF-16 code unit count.
///
/// LSP `Position.character` is measured in UTF-16 code units.  Given a string
/// and a byte offset into it, this returns how many UTF-16 units precede that
/// offset — which is the correct LSP character value.
pub(crate) fn byte_to_utf16(s: &str, byte_offset: usize) -> u32 {
    s[..byte_offset.min(s.len())]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum()
}

/// Count the UTF-16 code units in a string.
/// Needed for LSP `Position.character` calculations, which use UTF-16 offsets.
pub(crate) fn utf16_code_units(s: &str) -> u32 {
    s.chars().map(|c| c.len_utf16() as u32).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_to_utf16_ascii() {
        assert_eq!(byte_to_utf16("hello", 3), 3);
    }

    #[test]
    fn byte_to_utf16_multibyte_bmp() {
        // "é" is U+00E9: 2 bytes in UTF-8, 1 code unit in UTF-16.
        let s = "café";
        assert_eq!(byte_to_utf16(s, 0), 0);
        assert_eq!(byte_to_utf16(s, 3), 3); // up to "caf" (all ASCII)
        assert_eq!(byte_to_utf16(s, 5), 4); // full string (é = 2 bytes → 1 UTF-16 unit)
    }

    #[test]
    fn byte_to_utf16_surrogate_pair() {
        // "😀" is U+1F600: 4 bytes in UTF-8, 2 code units in UTF-16 (surrogate pair).
        let s = "a😀b";
        assert_eq!(byte_to_utf16(s, 1), 1); // after "a"
        assert_eq!(byte_to_utf16(s, 5), 3); // after "a😀" (emoji = 4 bytes → 2 UTF-16 units)
        assert_eq!(byte_to_utf16(s, 6), 4); // full string
    }

    #[test]
    fn byte_to_utf16_past_end_clamps() {
        assert_eq!(byte_to_utf16("hi", 100), 2);
    }

    #[test]
    fn utf16_offset_to_byte_ascii() {
        assert_eq!(utf16_offset_to_byte("hello", 3), 3);
    }

    #[test]
    fn utf16_offset_to_byte_surrogate_pair() {
        // "a😀b": UTF-16 offset 1 → byte 1 (start of emoji), offset 3 → byte 5 (after emoji)
        let s = "a😀b";
        assert_eq!(utf16_offset_to_byte(s, 1), 1);
        assert_eq!(utf16_offset_to_byte(s, 3), 5);
    }

    #[test]
    fn position_to_byte_offset_basic() {
        let s = "<?php\necho 1;\n";
        let p = |line, character| Position { line, character };
        assert_eq!(position_to_byte_offset(s, p(0, 0)), 0);
        assert_eq!(position_to_byte_offset(s, p(0, 5)), 5);
        assert_eq!(position_to_byte_offset(s, p(1, 0)), 6);
        assert_eq!(position_to_byte_offset(s, p(1, 4)), 10);
        // Column past end of line clamps to before the newline.
        assert_eq!(position_to_byte_offset(s, p(0, 99)), 5);
        // Line past end of text clamps to text length.
        assert_eq!(position_to_byte_offset(s, p(9, 0)), s.len());
    }

    #[test]
    fn position_to_byte_offset_multibyte() {
        // 😀 is one char, 4 UTF-8 bytes, 2 UTF-16 units.
        let s = "a😀b\nx";
        let p = |line, character| Position { line, character };
        assert_eq!(position_to_byte_offset(s, p(0, 1)), 1);
        assert_eq!(position_to_byte_offset(s, p(0, 3)), 5);
        assert_eq!(position_to_byte_offset(s, p(1, 0)), 7);
        assert_eq!(position_to_byte_offset(s, p(1, 1)), 8);
    }

    #[test]
    fn apply_content_change_replaces_inserts_deletes() {
        let r = |sl, sc, el, ec| Range {
            start: Position {
                line: sl,
                character: sc,
            },
            end: Position {
                line: el,
                character: ec,
            },
        };
        // Replacement within a line.
        let mut s = String::from("<?php\necho one;\n");
        apply_content_change(&mut s, r(1, 5, 1, 8), "two");
        assert_eq!(s, "<?php\necho two;\n");
        // Pure insertion (empty range).
        let mut s = String::from("ab\ncd\n");
        apply_content_change(&mut s, r(1, 1, 1, 1), "X");
        assert_eq!(s, "ab\ncXd\n");
        // Deletion spanning a newline (end position at start of next line).
        let mut s = String::from("ab\ncd\nef\n");
        apply_content_change(&mut s, r(0, 2, 1, 0), "");
        assert_eq!(s, "abcd\nef\n");
        // Malformed range (end before start) degrades to insertion.
        let mut s = String::from("abc");
        apply_content_change(&mut s, r(0, 2, 0, 1), "X");
        assert_eq!(s, "abXc");
    }

    #[test]
    fn byte_to_utf16_and_back_roundtrip() {
        let s = "café 😀 world";
        for (byte_idx, _) in s.char_indices() {
            let utf16 = byte_to_utf16(s, byte_idx) as usize;
            assert_eq!(utf16_offset_to_byte(s, utf16), byte_idx);
        }
    }
}
