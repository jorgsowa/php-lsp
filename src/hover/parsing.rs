use php_ast::{NamespaceBody, Stmt, StmtKind, UseKind};

use crate::util::fqn_short_name;

/// Extract the receiver variable from immediately before `->word` or `?->word`
/// at the cursor's exact column position.  Uses the column rather than
/// `str::find()` so multiple method calls on the same line are handled
/// correctly.
pub fn extract_receiver_var_before_cursor(line: &str, cursor_col_utf16: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();

    // Convert UTF-16 cursor column to char index.
    let mut utf16 = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16 >= cursor_col_utf16 {
            break;
        }
        utf16 += ch.len_utf16();
        char_idx += 1;
    }

    // Find the start of the word under the cursor (expand left).
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut word_start = char_idx;
    while word_start > 0 && is_word_char(chars[word_start - 1]) {
        word_start -= 1;
    }

    // Check for `?->` (3 chars) or `->` (2 chars) immediately before word_start.
    let (is_arrow, arrow_end) = if word_start >= 3
        && chars[word_start - 3] == '?'
        && chars[word_start - 2] == '-'
        && chars[word_start - 1] == '>'
    {
        (true, word_start - 3)
    } else if word_start >= 2 && chars[word_start - 2] == '-' && chars[word_start - 1] == '>' {
        (true, word_start - 2)
    } else {
        (false, 0)
    };

    if !is_arrow {
        return None;
    }

    extract_name_from_chars_end(&chars[..arrow_end])
}

/// Extract the class name from immediately before `::` at the cursor's column.
pub(crate) fn extract_static_class_before_cursor(
    line: &str,
    cursor_col_utf16: usize,
) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();

    let mut utf16 = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16 >= cursor_col_utf16 {
            break;
        }
        utf16 += ch.len_utf16();
        char_idx += 1;
    }

    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    let mut word_start = char_idx;
    while word_start > 0 && is_word_char(chars[word_start - 1]) {
        word_start -= 1;
    }

    // For `Class::$prop`, skip the `$` before checking for `::`
    if word_start > 0 && chars[word_start - 1] == '$' {
        word_start -= 1;
    }

    if word_start < 2 || chars[word_start - 2] != ':' || chars[word_start - 1] != ':' {
        return None;
    }

    let before_colons = &chars[..word_start - 2];
    // Class name may contain `\` for FQN; extract the short name (last segment).
    let is_name_char = |c: char| c.is_alphanumeric() || c == '_' || c == '\\';
    let end = before_colons.len().saturating_sub(
        before_colons
            .iter()
            .rev()
            .take_while(|&&c| c == ' ' || c == '\t')
            .count(),
    );
    let mut start = end;
    while start > 0 && is_name_char(before_colons[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let full: String = before_colons[start..end].iter().collect();
    // Return only the last segment so callers get a short name.
    Some(fqn_short_name(&full).to_owned())
}

/// Walk backwards through `chars`, skipping whitespace, and return the
/// identifier (with `$` prefix if present) ending at the last non-space char.
pub(crate) fn extract_name_from_chars_end(chars: &[char]) -> Option<String> {
    let is_var_char = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let end = chars.len()
        - chars
            .iter()
            .rev()
            .take_while(|&&c| c == ' ' || c == '\t')
            .count();
    if end == 0 {
        return None;
    }
    let mut start = end;
    while start > 0 && is_var_char(chars[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let name: String = chars[start..end].iter().collect();
    if name.starts_with('$') && name.len() > 1 {
        Some(name)
    } else if !name.is_empty() && !name.starts_with('$') {
        // Plain identifier (e.g. `$obj->getUser()->name` — the inner result):
        // treat as a non-variable receiver; callers handle the `$` lookup.
        Some(format!("${}", name))
    } else {
        None
    }
}

/// Resolve a use-import alias to the short class name.
///
/// Given `use App\Foo as Bar`, hovering on `Bar` anywhere in the file should
/// resolve to `Foo` so the declaration lookup succeeds.
pub fn resolve_use_alias(stmts: &[Stmt<'_, '_>], word: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => {
                for item in u.uses.iter() {
                    if let Some(alias) = item.alias
                        && alias == word
                    {
                        let fqn = item.name.to_string_repr();
                        let short = fqn_short_name(&fqn).to_owned();
                        return Some(short);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = resolve_use_alias(&inner.stmts, word)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}
