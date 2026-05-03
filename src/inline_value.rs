use tower_lsp::lsp_types::*;

/// Return `InlineValueVariableLookup` entries for every `$variable` occurrence
/// within `range` in `source`.
///
/// The debug adapter uses these to look up live variable values from the
/// runtime when execution is paused at a breakpoint.  We return every PHP
/// variable reference visible in the viewport so the adapter can fill them all
/// in without the language server needing debugger integration.
pub fn inline_values_in_range(source: &str, range: Range) -> Vec<InlineValue> {
    let mut result = Vec::new();

    // First-character predicate matches PHP's `[a-zA-Z_\x80-\xff]` — extended
    // here to any Unicode alphabetic char so multi-byte identifiers (PHP
    // sources are usually UTF-8 in practice) are scanned correctly.
    let is_ident_start = |c: char| c.is_alphabetic() || c == '_';
    let is_ident_cont = |c: char| c.is_alphanumeric() || c == '_';

    for (line_idx, line) in source.lines().enumerate() {
        let line_num = line_idx as u32;
        if line_num < range.start.line || line_num > range.end.line {
            continue;
        }
        // Per the LSP spec, the request is a Range — column boundaries on
        // the first and last line must be respected. Mid-range lines are
        // covered in full. Columns are UTF-16 code units.
        let line_min_col: Option<u32> =
            (line_num == range.start.line).then_some(range.start.character);
        let line_max_col: Option<u32> = (line_num == range.end.line).then_some(range.end.character);

        // Walk per-character so columns track UTF-16 code units correctly
        // even when the source contains multi-byte characters.
        let chars: Vec<(u32, char)> = {
            let mut out = Vec::with_capacity(line.len());
            let mut col: u32 = 0;
            for ch in line.chars() {
                out.push((col, ch));
                col += ch.len_utf16() as u32;
            }
            out
        };

        let mut i = 0usize;
        while i < chars.len() {
            if chars[i].1 != '$' {
                i += 1;
                continue;
            }
            // Skip `$$` (variable variables) — too dynamic to be useful.
            if chars.get(i + 1).map(|(_, c)| *c) == Some('$') {
                i += 2;
                continue;
            }
            let dollar_col = chars[i].0;
            i += 1;
            // Need at least one identifier-start character after the `$`.
            let Some(&(_, first)) = chars.get(i) else {
                continue;
            };
            if !is_ident_start(first) {
                continue;
            }
            let name_start_idx = i;
            while i < chars.len() && is_ident_cont(chars[i].1) {
                i += 1;
            }
            let name_end_idx = i;
            let var_name: String = chars[name_start_idx..name_end_idx]
                .iter()
                .map(|(_, c)| *c)
                .collect();

            // Omit `$this` — every method has it and it adds noise without value.
            if var_name == "this" {
                continue;
            }

            let end_col = chars.get(name_end_idx).map(|(c, _)| *c).unwrap_or_else(|| {
                chars
                    .last()
                    .map(|(c, ch)| c + ch.len_utf16() as u32)
                    .unwrap_or(0)
            });

            // Skip occurrences that fall outside the requested range's
            // column boundaries on the start/end lines.
            if let Some(min) = line_min_col
                && dollar_col < min
            {
                continue;
            }
            if let Some(max) = line_max_col
                && end_col > max
            {
                continue;
            }
            result.push(InlineValue::VariableLookup(InlineValueVariableLookup {
                range: Range {
                    start: Position {
                        line: line_num,
                        character: dollar_col,
                    },
                    end: Position {
                        line: line_num,
                        character: end_col,
                    },
                },
                // Provide the name without '$' so the DAP adapter can look it up
                // by name in the current stack frame.
                variable_name: Some(var_name),
                case_sensitive_lookup: true,
            }));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Range {
        Range {
            start: Position {
                line: sl,
                character: sc,
            },
            end: Position {
                line: el,
                character: ec,
            },
        }
    }

    #[test]
    fn finds_variables_in_range() {
        let src = "<?php\n$foo = 1;\n$bar = 2;\n";
        let vals = inline_values_in_range(src, range(1, 0, 2, 99));
        assert_eq!(vals.len(), 2);
        if let InlineValue::VariableLookup(v) = &vals[0] {
            assert_eq!(v.variable_name.as_deref(), Some("foo"));
            assert_eq!(v.range.start.line, 1);
        } else {
            panic!("expected VariableLookup");
        }
    }

    #[test]
    fn skips_this() {
        let src = "<?php\n$this->foo = $bar;";
        let vals = inline_values_in_range(src, range(1, 0, 1, 99));
        assert_eq!(vals.len(), 1);
        if let InlineValue::VariableLookup(v) = &vals[0] {
            assert_eq!(v.variable_name.as_deref(), Some("bar"));
        }
    }

    #[test]
    fn excludes_lines_outside_range() {
        let src = "<?php\n$x = 1;\n$y = 2;\n$z = 3;\n";
        let vals = inline_values_in_range(src, range(2, 0, 2, 99));
        assert_eq!(vals.len(), 1);
        if let InlineValue::VariableLookup(v) = &vals[0] {
            assert_eq!(v.variable_name.as_deref(), Some("y"));
        }
    }

    #[test]
    fn skips_variable_variables() {
        let src = "<?php\n$$dynamic = 1;";
        let vals = inline_values_in_range(src, range(1, 0, 1, 99));
        assert!(vals.is_empty(), "variable-variables should be skipped");
    }
}
