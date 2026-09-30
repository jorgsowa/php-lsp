use php_ast::{NamespaceBody, Stmt, StmtKind, UseKind};

use crate::text::fqn_short_name;

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

/// The cursor's member name follows `->`/`?->` on a receiver other than `$this`.
pub fn is_arrow_access_on_non_this_receiver(line: &str, cursor_col_utf16: usize) -> bool {
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
    let mut word_start = char_idx;
    while word_start > 0
        && (chars[word_start - 1].is_alphanumeric() || chars[word_start - 1] == '_')
    {
        word_start -= 1;
    }
    let arrow_end = if word_start >= 3 && chars[word_start - 3..word_start] == ['?', '-', '>'] {
        word_start - 3
    } else if word_start >= 2 && chars[word_start - 2..word_start] == ['-', '>'] {
        word_start - 2
    } else {
        return false;
    };
    let receiver: String = chars[..arrow_end].iter().collect();
    !receiver.trim_end().ends_with("$this")
}

/// Extract the class name from immediately before `::` at the cursor's column.
pub fn extract_static_class_before_cursor(line: &str, cursor_col_utf16: usize) -> Option<String> {
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
    resolve_use_alias_fqn(stmts, word).map(|(short, _)| short)
}

/// Like [`resolve_use_alias`], but also returns the full FQN the alias
/// resolves to, so callers can disambiguate between same-named classes in
/// different namespaces (e.g. many vendored `Factory` classes all locally
/// aliased to `FactoryContract`) instead of matching on the short name alone.
pub fn resolve_use_alias_fqn(stmts: &[Stmt<'_, '_>], word: &str) -> Option<(String, String)> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Use(u) if u.kind == UseKind::Normal => {
                for item in u.uses.iter() {
                    if let Some(alias) = item.alias
                        && alias == word
                    {
                        let fqn = item.name.to_string_repr().into_owned();
                        let short = fqn_short_name(&fqn).to_owned();
                        return Some((short, fqn));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(s) = resolve_use_alias_fqn(&inner.stmts, word)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::is_arrow_access_on_non_this_receiver as foreign;

    #[test]
    fn arrow_on_this_is_not_foreign() {
        assert!(!foreign("$this->run()", 8));
        assert!(!foreign("$this?->run()", 9));
    }

    #[test]
    fn arrow_on_property_or_variable_is_foreign() {
        assert!(foreign("$this->sut->run()", 13));
        assert!(foreign("$obj->run()", 7));
        assert!(foreign("$notthis->run()", 11));
    }

    #[test]
    fn non_arrow_positions_are_not_foreign() {
        assert!(!foreign("run()", 2));
        assert!(!foreign("Foo::run()", 7));
    }
}
