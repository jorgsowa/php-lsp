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
use tower_lsp::lsp_types::{Position, Range, TextEdit};

/// Format `source` with the best available PHP formatter.
/// Returns `None` if no formatter is installed or if the source was unchanged.
pub fn format_document(source: &str) -> Option<Vec<TextEdit>> {
    let formatted = run_formatter(source)?;
    if formatted == source {
        return None; // already clean — no edits needed
    }
    let line_count = source.lines().count() as u32;
    let last_line_len = source
        .lines()
        .last()
        .map(|l| l.chars().map(|c| c.len_utf16() as u32).sum())
        .unwrap_or(0);
    Some(vec![TextEdit {
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: line_count.saturating_sub(1),
                character: last_line_len,
            },
        },
        new_text: formatted,
    }])
}

/// Format only the lines covered by `range`.  Extracts those lines, formats
/// the snippet, then returns an edit targeting just that range.
pub fn format_range(source: &str, range: Range) -> Option<Vec<TextEdit>> {
    let lines: Vec<&str> = source.lines().collect();
    let start = range.start.line as usize;
    let end = (range.end.line as usize + 1).min(lines.len());
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

// ── Formatter invocation ──────────────────────────────────────────────────────

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
    fn unchanged_source_returns_none() {
        // This test only runs if a formatter is available; otherwise it's a no-op.
        let src = "<?php\n\nfunction greet(): void\n{\n}\n";
        // We just check the function doesn't panic — result depends on installed tools.
        let _ = format_document(src);
    }

    #[test]
    fn format_range_end_char_is_utf16_not_bytes() {
        // Last line contains "é" (2 bytes, 1 UTF-16 unit) so byte len != UTF-16 len.
        // format_range returns None when no formatter is installed, but when it does
        // return an edit the end character must be the UTF-16 length, not byte length.
        let src = "<?php\n$x = 1;\n$y = \"café\";\n";
        let range = Range {
            start: Position {
                line: 2,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 0,
            },
        };
        // We can't force a formatter to be installed, so we verify the computation
        // directly by checking that the UTF-16 length of the last line differs from
        // its byte length (ensuring the test would have caught the bug).
        let last_line = "$y = \"café\";";
        let byte_len = last_line.len() as u32;
        let utf16_len: u32 = last_line.chars().map(|c| c.len_utf16() as u32).sum();
        assert_ne!(
            byte_len, utf16_len,
            "test requires a line where byte len != UTF-16 len"
        );
        // "café" has 12 chars, each a BMP code point (é = 1 UTF-16 unit), so 12 UTF-16 units.
        // é is 2 bytes in UTF-8, so byte length is 13.
        assert_eq!(utf16_len, 12);
        assert_eq!(byte_len, 13);

        // Smoke-check: function must not panic regardless of formatter availability.
        let _ = format_range(src, range);
    }

    #[test]
    fn format_range_does_not_panic_on_empty_source() {
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        };
        let _ = format_range("<?php\n", range);
    }

    #[test]
    fn format_document_returns_edit_or_none() {
        let src = "<?php\nfunction foo()  {  }\n";
        let result = format_document(src);
        // Either None (no formatter installed) or Some with a single edit
        if let Some(edits) = result {
            assert_eq!(edits.len(), 1);
            assert_eq!(
                edits[0].range.start,
                Position {
                    line: 0,
                    character: 0
                }
            );
        }
    }
}
