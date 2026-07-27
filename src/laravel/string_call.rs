//! Text-based detection of a string-literal argument to a bare Laravel
//! helper call (`env('KEY')`, and — as later domains land — `config('a.b')`,
//! `view('a.b')`, `trans('a.b')`, `route('name')`).
//!
//! Mirrors `completion::include_path`'s line-scan approach rather than an AST
//! walk: these calls are conventionally single-line, and a line scan handles
//! the mid-edit (unterminated string literal) case completion needs for free.

use tower_lsp::lsp_types::{Position, Range};

use crate::text::{byte_to_utf16, utf16_offset_to_byte};

/// Full string-literal content (quotes excluded) and its `Range`, when the
/// cursor sits anywhere inside a *closed* string literal that is the first
/// argument of a bare call to one of `names` — e.g. cursor anywhere inside
/// `'APP_NAME'` in `env('APP_NAME')`.
pub(crate) fn call_string_arg(
    source: &str,
    position: Position,
    names: &[&str],
) -> Option<(String, Range)> {
    let lines: Vec<&str> = source.lines().collect();
    let line = *lines.get(position.line as usize)?;
    let byte_col = utf16_offset_to_byte(line, position.character as usize);
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let quote = bytes[i];
        if quote != b'\'' && quote != b'"' {
            i += 1;
            continue;
        }
        let content_start = i + 1;
        let mut j = content_start;
        while j < bytes.len() && bytes[j] != quote {
            j += 1;
        }
        if j >= bytes.len() {
            // Unterminated on this line — nothing more to scan.
            break;
        }
        if byte_col >= i && byte_col <= j {
            if !preceded_by_call_wrapped(&lines, position.line as usize, &line[..i], names) {
                return None;
            }
            let content = line[content_start..j].to_string();
            return Some((
                content,
                Range {
                    start: Position {
                        line: position.line,
                        character: byte_to_utf16(line, content_start),
                    },
                    end: Position {
                        line: position.line,
                        character: byte_to_utf16(line, j),
                    },
                },
            ));
        }
        i = j + 1;
    }
    None
}

/// Typed prefix (from the opening quote up to the cursor), when the cursor
/// sits inside a string literal — closed or not — immediately following a
/// bare call to one of `names`. Used for completion, where the closing quote
/// may not exist yet.
pub(crate) fn call_string_prefix(
    source: &str,
    position: Position,
    names: &[&str],
) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    let line = *lines.get(position.line as usize)?;
    let byte_col = utf16_offset_to_byte(line, position.character as usize);
    let before = &line[..byte_col];
    let quote_pos = before.rfind(['\'', '"'])?;
    if !preceded_by_call_wrapped(&lines, position.line as usize, &before[..quote_pos], names) {
        return None;
    }
    Some(before[quote_pos + 1..].to_string())
}

/// Every string-literal argument to a bare call to one of `names` anywhere
/// in `source` whose content equals `target`, with its `Range`. Used to
/// sweep a file for Laravel string-key usages once the key is already known
/// (find-references), as opposed to `call_string_arg`'s single
/// cursor-position lookup.
pub(crate) fn find_call_sites(source: &str, names: &[&str], target: &str) -> Vec<Range> {
    let mut out = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    for (line_no, line) in lines.iter().enumerate() {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let quote = bytes[i];
            if quote != b'\'' && quote != b'"' {
                i += 1;
                continue;
            }
            let content_start = i + 1;
            let mut j = content_start;
            while j < bytes.len() && bytes[j] != quote {
                j += 1;
            }
            if j >= bytes.len() {
                break;
            }
            if line[content_start..j] == *target
                && preceded_by_call_wrapped(&lines, line_no, &line[..i], names)
            {
                out.push(Range {
                    start: Position {
                        line: line_no as u32,
                        character: byte_to_utf16(line, content_start),
                    },
                    end: Position {
                        line: line_no as u32,
                        character: byte_to_utf16(line, j),
                    },
                });
            }
            i = j + 1;
        }
    }
    out
}

/// Same as `preceded_by_call`, but also recognizes a wrapped call — one
/// where the opening `name(` sits on an earlier line than the string
/// argument, a common shape after formatter line-wrapping for long
/// route/view/translation names:
/// ```php
/// route(
///     'admin.dashboard'
/// );
/// ```
/// Only kicks in when there is nothing but whitespace before the quote on
/// its own line — otherwise `before_quote` already carries the real answer.
fn preceded_by_call_wrapped(
    lines: &[&str],
    line_idx: usize,
    before_quote: &str,
    names: &[&str],
) -> bool {
    if preceded_by_call(before_quote, names) {
        return true;
    }
    if !before_quote.trim().is_empty() {
        return false;
    }
    let mut i = line_idx;
    while i > 0 {
        i -= 1;
        let prev = lines[i];
        if prev.trim().is_empty() {
            continue;
        }
        return preceded_by_call(prev, names);
    }
    false
}

/// Whether `before_quote` (the line text up to, but excluding, the opening
/// quote) ends with a bare call to one of `names` — `env(`, `config(`, etc.
/// — at a word boundary, so `getenv(` doesn't match the `env` pattern.
fn preceded_by_call(before_quote: &str, names: &[&str]) -> bool {
    let Some(rest) = before_quote.trim_end().strip_suffix('(') else {
        return false;
    };
    let rest = rest.trim_end();
    names.iter().any(|name| {
        rest.len() >= name.len()
            && rest[rest.len() - name.len()..].eq_ignore_ascii_case(name)
            && word_boundary_before(rest, rest.len() - name.len())
    })
}

fn word_boundary_before(s: &str, idx: usize) -> bool {
    idx == 0 || !matches!(s.as_bytes()[idx - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV: &[&str] = &["env"];

    #[test]
    fn call_string_arg_matches_env_call() {
        let src = "<?php\n$x = env('APP_NAME');\n";
        // Cursor inside "APP_NAME".
        let pos = Position {
            line: 1,
            character: 15,
        };
        let (content, range) = call_string_arg(src, pos, ENV).unwrap();
        assert_eq!(content, "APP_NAME");
        assert_eq!(
            range.start,
            Position {
                line: 1,
                character: 10
            }
        );
        assert_eq!(
            range.end,
            Position {
                line: 1,
                character: 18
            }
        );
    }

    #[test]
    fn call_string_arg_rejects_unrelated_call() {
        let src = "<?php\n$x = getenv('APP_NAME');\n";
        let pos = Position {
            line: 1,
            character: 18,
        };
        assert!(call_string_arg(src, pos, ENV).is_none());
    }

    #[test]
    fn call_string_arg_rejects_plain_string_containing_pattern_textually() {
        let src = "<?php\n$x = 'env(APP_NAME)';\n";
        let pos = Position {
            line: 1,
            character: 12,
        };
        assert!(call_string_arg(src, pos, ENV).is_none());
    }

    #[test]
    fn call_string_arg_allows_whitespace_before_paren_and_quote() {
        let src = "<?php\n$x = env( 'APP_NAME' );\n";
        let pos = Position {
            line: 1,
            character: 16,
        };
        let (content, _) = call_string_arg(src, pos, ENV).unwrap();
        assert_eq!(content, "APP_NAME");
    }

    #[test]
    fn call_string_prefix_returns_typed_text() {
        let src = "<?php\nenv('APP_N";
        let pos = Position {
            line: 1,
            character: 11,
        };
        assert_eq!(call_string_prefix(src, pos, ENV).as_deref(), Some("APP_N"));
    }

    #[test]
    fn call_string_prefix_none_for_other_call() {
        let src = "<?php\nconfig('APP_N";
        let pos = Position {
            line: 1,
            character: 14,
        };
        assert!(call_string_prefix(src, pos, ENV).is_none());
    }

    #[test]
    fn call_string_prefix_empty_right_after_quote() {
        let src = "<?php\nenv('";
        let pos = Position {
            line: 1,
            character: 5,
        };
        assert_eq!(call_string_prefix(src, pos, ENV).as_deref(), Some(""));
    }

    #[test]
    fn find_call_sites_collects_every_matching_call_across_lines() {
        let src = "<?php\n$a = env('APP_NAME');\n$b = env('APP_NAME');\n$c = env('OTHER');\n";
        let sites = find_call_sites(src, ENV, "APP_NAME");
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].start.line, 1);
        assert_eq!(sites[1].start.line, 2);
    }

    #[test]
    fn find_call_sites_ignores_unrelated_calls_and_keys() {
        let src = "<?php\n$a = getenv('APP_NAME');\n$b = env('OTHER');\n";
        assert!(find_call_sites(src, ENV, "APP_NAME").is_empty());
    }

    #[test]
    fn find_call_sites_empty_for_no_matches() {
        let src = "<?php\necho 'hello';\n";
        assert!(find_call_sites(src, ENV, "APP_NAME").is_empty());
    }

    #[test]
    fn call_string_arg_matches_wrapped_call() {
        let src = "<?php\nenv(\n    'APP_NAME'\n);\n";
        // Cursor inside "APP_NAME" on its own line.
        let pos = Position {
            line: 2,
            character: 8,
        };
        let (content, _) = call_string_arg(src, pos, ENV).unwrap();
        assert_eq!(content, "APP_NAME");
    }

    #[test]
    fn call_string_arg_wrapped_call_skips_blank_lines() {
        let src = "<?php\nenv(\n\n    'APP_NAME'\n);\n";
        let pos = Position {
            line: 3,
            character: 8,
        };
        let (content, _) = call_string_arg(src, pos, ENV).unwrap();
        assert_eq!(content, "APP_NAME");
    }

    #[test]
    fn call_string_arg_wrapped_call_rejects_unrelated_call() {
        let src = "<?php\ngetenv(\n    'APP_NAME'\n);\n";
        let pos = Position {
            line: 2,
            character: 8,
        };
        assert!(call_string_arg(src, pos, ENV).is_none());
    }

    #[test]
    fn find_call_sites_matches_wrapped_call() {
        let src = "<?php\nenv(\n    'APP_NAME'\n);\n";
        let sites = find_call_sites(src, ENV, "APP_NAME");
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].start.line, 2);
    }
}
