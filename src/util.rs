use tower_lsp::lsp_types::Position;

/// Extract the word (identifier) under the cursor, handling UTF-16 offsets.
pub(crate) fn word_at(source: &str, position: Position) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let char_offset = position.character as usize;

    let chars: Vec<char> = line.chars().collect();

    let mut utf16_len = 0usize;
    let mut char_pos = 0usize;
    for ch in &chars {
        if utf16_len >= char_offset {
            break;
        }
        utf16_len += ch.len_utf16();
        char_pos += 1;
    }

    let total_utf16: usize = chars.iter().map(|c| c.len_utf16()).sum();
    if char_offset > total_utf16 {
        return None;
    }

    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '\\';

    let mut left = char_pos;
    while left > 0 && is_word(chars[left - 1]) {
        left -= 1;
    }

    let mut right = char_pos;
    while right < chars.len() && is_word(chars[right]) {
        right += 1;
    }

    if left == right {
        return None;
    }

    let word: String = chars[left..right].iter().collect();
    if word.is_empty() { None } else { Some(word) }
}
