//! The file header: the lines before the first declaration or statement, where
//! `<?php`, `declare`, `namespace` and import `use` statements live.

/// Number of leading lines of `source` that belong to the header. A `use` inside
/// a class body (trait-use) or after any code is never part of it.
pub(crate) fn header_line_count(source: &str) -> usize {
    let mut in_use_stmt = false;
    let mut in_block_comment = false;
    for (idx, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if in_block_comment {
            in_block_comment = !trimmed.contains("*/");
            continue;
        }
        if in_use_stmt {
            in_use_stmt = !trimmed.contains(';');
            continue;
        }
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || (trimmed.starts_with('#') && !trimmed.starts_with("#["))
        {
            continue;
        }
        if trimmed.starts_with("/*") {
            in_block_comment = !trimmed.contains("*/");
            continue;
        }
        if trimmed.starts_with("<?php")
            || is_keyword_stmt(trimmed, "declare")
            || is_keyword_stmt(trimmed, "namespace")
        {
            continue;
        }
        if is_keyword_stmt(trimmed, "use") {
            in_use_stmt = !trimmed.contains(';');
            continue;
        }
        return idx;
    }
    source.lines().count()
}

/// Lines of the header as `(line number, line)` pairs.
pub(crate) fn header_lines(source: &str) -> impl Iterator<Item = (u32, &str)> {
    source
        .lines()
        .take(header_line_count(source))
        .enumerate()
        .map(|(i, l)| (i as u32, l))
}

fn is_keyword_stmt(trimmed: &str, keyword: &str) -> bool {
    trimmed
        .strip_prefix(keyword)
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_whitespace() || c == '(' || c == '{'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_stops_at_class() {
        let src = "<?php\nnamespace App;\n\nuse Foo;\n\nclass A {\n    use T;\n}\n";
        assert_eq!(header_line_count(src), 5);
    }

    #[test]
    fn header_skips_docblocks_and_multiline_group_use() {
        let src = "<?php\n/**\n * use Nope;\n */\nuse App\\{\n    A,\n    B\n};\nfunction f() {}\n";
        assert_eq!(header_line_count(src), 8);
    }

    #[test]
    fn namespace_separator_is_not_a_namespace_statement() {
        assert!(!is_keyword_stmt("namespaced();", "namespace"));
        assert!(!is_keyword_stmt("namespace\\foo();", "namespace"));
    }
}
