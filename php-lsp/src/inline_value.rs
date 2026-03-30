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

    for (line_idx, line) in source.lines().enumerate() {
        let line_num = line_idx as u32;
        if line_num < range.start.line || line_num > range.end.line {
            continue;
        }

        let bytes = line.as_bytes();
        let mut i = 0usize;

        while i < bytes.len() {
            if bytes[i] != b'$' {
                i += 1;
                continue;
            }

            // Skip `$$` (variable variables) — too dynamic to be useful.
            if bytes.get(i + 1) == Some(&b'$') {
                i += 2;
                continue;
            }

            let dollar_col = i as u32;
            i += 1; // move past '$'

            // Collect [a-zA-Z_\x80-\xff][a-zA-Z0-9_\x80-\xff]*
            if i >= bytes.len()
                || !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' || bytes[i] >= 0x80)
            {
                continue;
            }
            let name_start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 0x80)
            {
                i += 1;
            }
            let var_name = &line[name_start..i];

            // Omit `$this` — every method has it and it adds noise without value.
            if var_name == "this" {
                continue;
            }

            let end_col = i as u32;
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
                variable_name: Some(var_name.to_string()),
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
