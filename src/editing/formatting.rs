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
    let lines: Vec<&str> = source.lines().collect();
    let end = (range.end.line as usize + 1).min(lines.len());
    let start = (range.start.line as usize).min(end);
    if start == end {
        return None;
    }
    let snippet = lines[start..end].join("\n") + "\n";

    // Wrap in `<?php` if the snippet doesn't have an opener (needed for
    // php-cs-fixer to recognise it as PHP).
    let needs_wrapper = !snippet.trim_start().starts_with("<?php");
    let to_format = if needs_wrapper {
        format!("<?php\n{snippet}")
    } else {
        snippet.clone()
    };

    let mut formatted = run_formatter(&to_format)?;
    if needs_wrapper {
        // Strip the injected <?php header back out
        formatted = formatted
            .strip_prefix("<?php\n")
            .unwrap_or(&formatted)
            .to_string();
    }

    if formatted == snippet {
        return None;
    }

    let end_char = lines
        .get(end - 1)
        .map(|l| l.chars().map(|c| c.len_utf16() as u32).sum())
        .unwrap_or(0);
    Some(vec![TextEdit {
        range: Range {
            start: Position {
                line: range.start.line,
                character: 0,
            },
            end: Position {
                line: range.end.line,
                character: end_char,
            },
        },
        new_text: formatted,
    }])
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
}
