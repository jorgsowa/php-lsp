use tower_lsp::lsp_types::Position;

/// Strip the `$0` cursor marker from a PHP source string and return
/// the cleaned source together with the `Position` of that marker.
///
/// Useful in tests to avoid computing line/character offsets by hand:
///
/// ```rust
/// let (src, pos) = cursor("<?php\nfunction foo$0() {}");
/// // src == "<?php\nfunction foo() {}"
/// // pos == Position { line: 1, character: 12 }
/// ```
///
/// Panics if there is no `$0` in `src`.
pub fn cursor(src: &str) -> (String, Position) {
    const MARKER: &str = "$0";
    let marker_byte = src.find(MARKER).expect("no `$0` cursor marker in source");
    let before = &src[..marker_byte];
    let line = before.chars().filter(|&c| c == '\n').count() as u32;
    let last_nl_end = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    // LSP positions are UTF-16 code units
    let character = before[last_nl_end..].encode_utf16().count() as u32;
    let cleaned = format!("{}{}", before, &src[marker_byte + MARKER.len()..]);
    (cleaned, Position { line, character })
}
