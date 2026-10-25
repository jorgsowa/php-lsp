/// `textDocument/formatting` and `textDocument/rangeFormatting`.
///
/// Delegates to the first available PHP formatter found on `$PATH`:
///   1. `php-cs-fixer` (preferred — PSR-12 rules)
///   2. `phpcbf` (PHP_CodeSniffer)
///
/// If neither tool is found the handler returns `Ok(None)` so the editor
/// shows a gentle "formatter not available" message rather than an error.
///
/// Both handlers write the source to a temporary file, run the formatter
/// in-place, read the result, then return a single `TextEdit` that replaces
/// the entire document (simplest correct approach for whole-file formatting).
/// Range formatting narrows the edit to the requested line span.
use std::process::{Command, Stdio};
use tower_lsp_server::ls_types::{Position, Range, TextEdit};

use crate::text::end_position;

/// Format `source` with the best available PHP formatter.
/// Returns `None` if no formatter is installed or if the source was unchanged.
pub fn format_document(source: &str) -> Option<Vec<TextEdit>> {
    whole_document_edit(source, run_formatter(source)?)
}

fn whole_document_edit(source: &str, formatted: String) -> Option<Vec<TextEdit>> {
    if formatted == source {
        return None;
    }
    Some(vec![TextEdit {
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: end_position(source),
        },
        new_text: formatted,
    }])
}

/// Format only the lines covered by `range`.  Extracts those lines, formats
/// the snippet, then returns an edit targeting just that range.
pub fn format_range(source: &str, range: Range) -> Option<Vec<TextEdit>> {
    range_edit(source, range, run_formatter)
}

fn range_edit(
    source: &str,
    range: Range,
    format: impl Fn(&str) -> Option<String>,
) -> Option<Vec<TextEdit>> {
    // A selection ending at column 0 of a later line doesn't cover that line.
    let last_line = if range.end.character == 0 && range.end.line > range.start.line {
        range.end.line - 1
    } else {
        range.end.line
    };
    let start = line_start_byte(source, range.start.line)?;
    let end = line_start_byte(source, last_line + 1).unwrap_or(source.len());
    let snippet = &source[start..end];
    if snippet.trim().is_empty() {
        return None;
    }

    let crlf = snippet.contains("\r\n");
    let lf_snippet = snippet.replace("\r\n", "\n");
    let formatted = if lf_snippet.trim_start().starts_with("<?php") {
        format(&lf_snippet)?
    } else {
        format_fragment(&lf_snippet, &format)?
    };
    let mut formatted = formatted.trim_end_matches('\n').to_string();
    if lf_snippet.ends_with('\n') {
        formatted.push('\n');
    }
    if crlf {
        formatted = formatted.replace('\n', "\r\n");
    }
    if formatted == snippet {
        return None;
    }

    let end_pos = if end == source.len() {
        end_position(source)
    } else {
        Position {
            line: last_line + 1,
            character: 0,
        }
    };
    Some(vec![TextEdit {
        range: Range {
            start: Position {
                line: range.start.line,
                character: 0,
            },
            end: end_pos,
        },
        new_text: formatted,
    }])
}

/// Format a snippet without an opening tag: dedent it, format it behind an
/// injected `<?php`, then restore the original indentation.
fn format_fragment(snippet: &str, format: &impl Fn(&str) -> Option<String>) -> Option<String> {
    let indent = snippet
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| &l[..l.len() - l.trim_start().len()])
        .min_by_key(|ws| ws.len())
        .unwrap_or("");
    let dedented: String = snippet
        .lines()
        .map(|l| l.strip_prefix(indent).unwrap_or(l.trim_start()))
        .collect::<Vec<_>>()
        .join("\n");
    let formatted = format(&format!("<?php\n{dedented}\n"))?;
    // PSR-12 puts a blank line after `<?php`; it belongs to the wrapper, not the snippet.
    let body = formatted.strip_prefix("<?php")?.trim_start_matches('\n');
    let leading_blank_lines = snippet.len() - snippet.trim_start_matches('\n').len();
    let mut out = "\n".repeat(leading_blank_lines);
    for line in body.lines() {
        if !line.is_empty() {
            out.push_str(indent);
        }
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

/// Byte offset where line `line` starts, or `None` past the last line.
fn line_start_byte(source: &str, line: u32) -> Option<usize> {
    if line == 0 {
        return Some(0);
    }
    source
        .match_indices('\n')
        .nth(line as usize - 1)
        .map(|(i, _)| i + 1)
        .filter(|&i| i < source.len())
}

fn run_formatter(source: &str) -> Option<String> {
    try_php_cs_fixer(source).or_else(|| try_phpcbf(source))
}

fn try_php_cs_fixer(source: &str) -> Option<String> {
    // php-cs-fixer reads from stdin when passed `-` as the path.
    // `--dry-run` is NOT used so the formatter actually rewrites the content,
    // but since we pipe through stdin/stdout nothing on disk is touched.
    //
    // We use `fix --quiet --rules=@PSR12 -` which outputs the fixed source on
    // stdout when stdin mode is supported (php-cs-fixer ≥ 3.x).
    let output = Command::new("php-cs-fixer")
        .args(["fix", "--quiet", "--no-interaction", "--rules=@PSR12", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take()?.write_all(source.as_bytes()).ok()?;
            child.wait_with_output().ok()
        })?;

    if output.status.success() || output.status.code() == Some(1) {
        // exit 0 = already formatted, exit 1 = fixed
        let text = String::from_utf8(output.stdout).ok()?;
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

fn try_phpcbf(source: &str) -> Option<String> {
    // phpcbf writes to stdout when passed `--stdin-path` and reads from stdin.
    let output = Command::new("phpcbf")
        .args(["--standard=PSR12", "--stdin-path=file.php", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take()?.write_all(source.as_bytes()).ok()?;
            child.wait_with_output().ok()
        })?;

    // phpcbf exits 1 on success (fixable issues found and fixed), 0 = nothing to fix
    if output.status.code() == Some(1) || output.status.success() {
        let text = String::from_utf8(output.stdout).ok()?;
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_range_past_end_of_document_returns_none() {
        let range = Range {
            start: Position {
                line: 10,
                character: 0,
            },
            end: Position {
                line: 12,
                character: 0,
            },
        };
        assert_eq!(format_range("<?php\n$a = 1;\n", range), None);
        assert_eq!(format_range("", range), None);
    }

    fn apply(source: &str, edits: &[TextEdit]) -> String {
        let mut text = source.to_string();
        for edit in edits.iter().rev() {
            crate::text::apply_content_change(&mut text, edit.range, &edit.new_text);
        }
        text
    }

    #[test]
    fn whole_document_edit_replaces_trailing_newline() {
        let source = "<?php\nfunction f( $x ){}\n";
        let formatted = "<?php\nfunction f($x) {}\n";
        let edits = whole_document_edit(source, formatted.to_string()).unwrap();
        assert_eq!(apply(source, &edits), formatted);
        assert_eq!(whole_document_edit(formatted, formatted.to_string()), None);
    }

    #[test]
    fn whole_document_edit_covers_crlf_source() {
        let source = "<?php\r\nfunction f( $x ){}\r\n";
        let formatted = "<?php\r\nfunction f($x) {}\r\n";
        let edits = whole_document_edit(source, formatted.to_string()).unwrap();
        assert_eq!(apply(source, &edits), formatted);
    }

    fn range(start_line: u32, end_line: u32, end_char: u32) -> Range {
        Range {
            start: Position {
                line: start_line,
                character: 0,
            },
            end: Position {
                line: end_line,
                character: end_char,
            },
        }
    }

    /// Stands in for php-cs-fixer: fixes `( $x )` spacing and, like PSR-12, puts a
    /// blank line after `<?php` and ends the file with one newline.
    fn fake_formatter(src: &str) -> Option<String> {
        let body = src.strip_prefix("<?php\n")?.trim_start_matches('\n');
        let body = body.replace("( $x )", "($x)");
        Some(format!("<?php\n\n{}\n", body.trim_end()))
    }

    #[test]
    fn range_edit_keeps_single_newline_and_indent() {
        let source = "<?php\nclass A {\n    function f( $x ) {}\n    function g() {}\n}\n";
        let edits = range_edit(source, range(2, 2, 10), fake_formatter).unwrap();
        assert_eq!(
            apply(source, &edits),
            "<?php\nclass A {\n    function f($x) {}\n    function g() {}\n}\n"
        );
    }

    #[test]
    fn range_edit_preserves_crlf() {
        let source = "<?php\r\nf( $x );\r\ng( $x );\r\n";
        let edits = range_edit(source, range(1, 3, 0), fake_formatter).unwrap();
        assert_eq!(apply(source, &edits), "<?php\r\nf($x);\r\ng($x);\r\n");
    }

    #[test]
    fn range_edit_last_line_without_newline() {
        let source = "<?php\nf( $x );";
        let edits = range_edit(source, range(1, 1, 3), fake_formatter).unwrap();
        assert_eq!(apply(source, &edits), "<?php\nf($x);");
    }

    #[test]
    fn range_edit_unchanged_returns_none() {
        let source = "<?php\n    f($x);\n";
        assert_eq!(range_edit(source, range(1, 1, 5), fake_formatter), None);
    }
}
