//! Multi-file fixture DSL for integration tests.
//!
//! Lets a single string describe a whole workspace:
//!
//! ```text
//! //- /src/Greeter.php
//! <?php
//! class Greeter {
//!     public function hello(): string { return 'hi'; }
//! }
//!
//! //- /src/main.php
//! <?php
//! $g = new Greeter();
//! $g->hel$0lo();
//! ```
//!
//! Plus inline diagnostic annotations pointing at the previous code line:
//!
//! ```text
//! <?php
//! nonexistent_function();
//! // ^^^^^^^^^^^^^^^^^^^^ error: function is not defined
//! ```
//!
//! Without any `//- /path` header the whole input is treated as a single file
//! named `main.php`.

use serde_json::Value;

/// A parsed fixture: one or more files, optionally a `$0` cursor or a pair
/// of `$0` markers describing a range, and any inline diagnostic annotations
/// discovered per file.
///
/// Exactly one `$0` → `cursor` is set.
/// Exactly two `$0` → `range` is set (first is start, second is end); `cursor`
/// is also set to the start so tests that only need a position work uniformly.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub files: Vec<FixtureFile>,
    pub cursor: Option<Cursor>,
    pub range: Option<Range>,
}

/// Range spanning a selection in a single file. Both ends carry the same path.
#[derive(Debug, Clone)]
pub struct Range {
    pub path: String,
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
}

#[derive(Debug, Clone)]
pub struct FixtureFile {
    /// Path relative to workspace root, no leading slash (`"src/main.php"`).
    pub path: String,
    /// File contents with annotation lines and `$0` removed.
    pub text: String,
    /// Inline `// ^^^ …` annotations attached to this file.
    pub annotations: Vec<DiagnosticAnnotation>,
}

#[derive(Debug, Clone)]
pub struct Cursor {
    pub path: String,
    pub line: u32,
    pub character: u32,
}

/// One `// ^^^ severity: message` annotation. Line/columns are in the
/// post-strip coordinates of the owning file, matching what the server emits
/// in `publishDiagnostics`.
#[derive(Debug, Clone)]
pub struct DiagnosticAnnotation {
    pub line: u32,
    pub start_char: u32,
    pub end_char: u32,
    pub severity: String,
    pub message: String,
}

pub fn parse(src: &str) -> Fixture {
    let chunks = split_files(src);
    let mut files = Vec::with_capacity(chunks.len());
    let mut cursor: Option<Cursor> = None;
    let mut range: Option<Range> = None;

    for (path, raw) in chunks {
        let (text_with_annos, markers) = extract_cursors(&raw);
        // Build a line-shift map: each index is a pre-strip line number, the
        // value is how many annotation lines at-or-before that index will be
        // removed. Cursor line numbers collected from `text_with_annos` need
        // to be shifted by this count so they still address the same source
        // line after annotation lines are dropped from the output text.
        let shifts = annotation_line_shifts(&text_with_annos);
        let shift_for = |line: u32| -> u32 {
            shifts
                .get(line as usize)
                .copied()
                .unwrap_or_else(|| shifts.last().copied().unwrap_or(0))
        };
        match markers.len() {
            0 => {}
            1 => {
                assert!(
                    cursor.is_none(),
                    "fixture has $0 markers in more than one file (second in {path})"
                );
                let (l, c) = markers[0];
                cursor = Some(Cursor {
                    path: path.clone(),
                    line: l - shift_for(l),
                    character: c,
                });
            }
            2 => {
                assert!(
                    range.is_none() && cursor.is_none(),
                    "fixture has more than one $0 selection/cursor"
                );
                let (sl, sc) = markers[0];
                let (el, ec) = markers[1];
                range = Some(Range {
                    path: path.clone(),
                    start_line: sl - shift_for(sl),
                    start_character: sc,
                    end_line: el - shift_for(el),
                    end_character: ec,
                });
                cursor = Some(Cursor {
                    path: path.clone(),
                    line: sl - shift_for(sl),
                    character: sc,
                });
            }
            n => panic!("fixture has {n} $0 markers in {path}; expected 0, 1, or 2"),
        }
        let (text, annotations) = extract_annotations(&text_with_annos);
        files.push(FixtureFile {
            path,
            text,
            annotations,
        });
    }

    Fixture {
        files,
        cursor,
        range,
    }
}

/// Split raw input at `//- /path` header lines. If no header appears, the
/// whole input becomes a single `main.php` file.
fn split_files(src: &str) -> Vec<(String, String)> {
    let has_header = src.lines().any(|l| l.trim_start().starts_with("//- /"));
    if !has_header {
        return vec![("main.php".to_owned(), src.to_owned())];
    }

    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("//- /") {
            if let Some(prev) = current.take() {
                out.push(prev);
            }
            let path = rest
                .trim_end_matches(|c: char| c == '\n' || c == '\r')
                .trim();
            current = Some((path.to_owned(), String::new()));
        } else if let Some((_, buf)) = current.as_mut() {
            buf.push_str(line);
        }
        // Lines before the first header are discarded (e.g. blank preamble).
    }
    if let Some(prev) = current.take() {
        out.push(prev);
    }

    // Strip one leading blank-or-empty line per file so the file body aligns
    // naturally under the `//- /path` header.
    for (_, buf) in out.iter_mut() {
        if buf.starts_with('\n') {
            buf.remove(0);
        } else if buf.starts_with("\r\n") {
            buf.drain(..2);
        }
    }
    out
}

/// Remove every `$0` occurrence, returning the cleaned text and the
/// (line, character) of each marker in output coordinates (markers that appear
/// later still report coordinates post-strip of the earlier ones, which is
/// what a human reading the fixture expects). Character counts are UTF-16
/// code units to match LSP.
fn extract_cursors(src: &str) -> (String, Vec<(u32, u32)>) {
    let mut out = String::with_capacity(src.len());
    let mut markers = Vec::new();
    let mut i = 0;
    let bytes = src.as_bytes();
    let mut line: u32 = 0;
    let mut line_start_in_out: usize = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'0' {
            let col = out[line_start_in_out..].encode_utf16().count() as u32;
            markers.push((line, col));
            i += 2;
            continue;
        }
        let ch = bytes[i] as char;
        if ch == '\n' {
            out.push('\n');
            line += 1;
            line_start_in_out = out.len();
            i += 1;
        } else {
            // Preserve multi-byte chars.
            let ch_len = src[i..].chars().next().unwrap().len_utf8();
            out.push_str(&src[i..i + ch_len]);
            i += ch_len;
        }
    }
    (out, markers)
}

/// For each 0-based line index in `src`, the number of annotation lines
/// that appear at-or-before it (i.e. how many lines will disappear when
/// `extract_annotations` strips them out). Used to shift cursor coordinates
/// that were captured before annotation stripping.
fn annotation_line_shifts(src: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut shift: u32 = 0;
    for line in src.split('\n') {
        if parse_annotation_line(line, Some(0)).is_some() {
            shift += 1;
        }
        out.push(shift);
    }
    out
}

/// Pull `// ^^^ …` annotation lines out of `src` and return the stripped
/// source plus the attached annotations. Each annotation targets the nearest
/// non-annotation line above it.
fn extract_annotations(src: &str) -> (String, Vec<DiagnosticAnnotation>) {
    let mut kept_lines: Vec<&str> = Vec::new();
    let mut annotations: Vec<DiagnosticAnnotation> = Vec::new();
    // Maps an original-source line index to its index in `kept_lines` (so we
    // can resolve an annotation's target line in output coordinates).
    let mut last_kept_output_line: Option<u32> = None;

    for line in src.split('\n') {
        if let Some(anno) = parse_annotation_line(line, last_kept_output_line) {
            annotations.push(anno);
        } else {
            last_kept_output_line = Some(kept_lines.len() as u32);
            kept_lines.push(line);
        }
    }
    (kept_lines.join("\n"), annotations)
}

fn parse_annotation_line(line: &str, target_line: Option<u32>) -> Option<DiagnosticAnnotation> {
    // Expected shape: <indent>//<ws>^^^<ws>[severity:] message
    //
    // Bytes / UTF-16 don't diverge here: annotation lines are ASCII by
    // construction (`//`, whitespace, `^`, message). Non-ASCII is only
    // permitted on the *code* lines, which this function never reads.
    let rest = line.trim_start();
    let indent_len = line.len() - rest.len();
    let rest = rest.strip_prefix("//")?;
    let after_slashes = rest.trim_start();
    if !after_slashes.starts_with('^') {
        return None;
    }
    let ws_before_carets = rest.len() - after_slashes.len();
    let carets_len = after_slashes.chars().take_while(|c| *c == '^').count();
    let payload = after_slashes[carets_len..].trim_start();

    let target_line = target_line?;
    let start_char = (indent_len + 2 + ws_before_carets) as u32;
    let end_char = start_char + carets_len as u32;

    let (severity, message) = match payload.find(':') {
        Some(i) if is_severity_word(&payload[..i]) => {
            (payload[..i].to_owned(), payload[i + 1..].trim().to_owned())
        }
        _ => ("error".to_owned(), payload.trim().to_owned()),
    };

    Some(DiagnosticAnnotation {
        line: target_line,
        start_char,
        end_char,
        severity,
        message,
    })
}

/// Feature-agnostic view of a caret annotation. Carries the raw post-carets
/// payload (e.g. `"def"`, `"ref"`, `"read"`, `"write"`) — features pick their
/// own vocabulary. Reuses `DiagnosticAnnotation`'s storage: when no severity
/// prefix is present, `severity` is the default `"error"` and `message` holds
/// the full payload; that matches what navigation tests want directly.
pub fn generic_message(a: &DiagnosticAnnotation) -> &str {
    &a.message
}

fn is_severity_word(s: &str) -> bool {
    matches!(
        s.trim(),
        "error" | "warning" | "warn" | "info" | "information" | "hint"
    )
}

pub fn severity_number(name: &str) -> u64 {
    match name {
        "error" => 1,
        "warning" | "warn" => 2,
        "info" | "information" => 3,
        "hint" => 4,
        _ => 1,
    }
}

/// Assert that the server's `publishDiagnostics` payload for one file
/// contains exactly the annotations parsed from the fixture. Order-
/// independent: each annotation must match at least one diagnostic on range
/// + severity, and the diagnostic's `message` must *contain* the expected
/// message substring. Extra diagnostics not covered by annotations cause a
/// failure — annotations are a full specification of what we expect.
#[track_caller]
pub fn assert_diagnostics(notif: &Value, expected: &[DiagnosticAnnotation]) {
    let empty: Vec<Value> = Vec::new();
    let diags = notif["params"]["diagnostics"].as_array().unwrap_or(&empty);

    let mut matched = vec![false; diags.len()];
    let mut missing: Vec<&DiagnosticAnnotation> = Vec::new();

    for anno in expected {
        let sev = severity_number(&anno.severity);
        let hit = diags.iter().enumerate().position(|(i, d)| {
            if matched[i] {
                return false;
            }
            let r = &d["range"];
            let line_ok = r["start"]["line"].as_u64() == Some(anno.line as u64);
            let start_ok = r["start"]["character"].as_u64() == Some(anno.start_char as u64);
            let end_line_ok = r["end"]["line"].as_u64() == Some(anno.line as u64);
            let end_ok = r["end"]["character"].as_u64() == Some(anno.end_char as u64);
            let sev_ok = d["severity"].as_u64() == Some(sev);
            let msg_ok = anno.message.is_empty()
                || d["message"]
                    .as_str()
                    .map(|m| m.contains(&anno.message))
                    .unwrap_or(false);
            line_ok && start_ok && end_line_ok && end_ok && sev_ok && msg_ok
        });
        match hit {
            Some(i) => matched[i] = true,
            None => missing.push(anno),
        }
    }

    let extras: Vec<&Value> = diags
        .iter()
        .enumerate()
        .filter(|(i, _)| !matched[*i])
        .map(|(_, d)| d)
        .collect();

    if !missing.is_empty() || !extras.is_empty() {
        panic!(
            "diagnostic mismatch\n\
             expected (missing): {missing:#?}\n\
             actual (unmatched): {extras:#?}\n\
             full payload: {payload}",
            payload = notif["params"]["diagnostics"],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_file_no_header() {
        let f = parse("<?php\necho 1;\n");
        assert_eq!(f.files.len(), 1);
        assert_eq!(f.files[0].path, "main.php");
        assert_eq!(f.files[0].text, "<?php\necho 1;\n");
        assert!(f.cursor.is_none());
    }

    #[test]
    fn multi_file_split() {
        let src = "//- /a.php\n<?php\nA;\n//- /b.php\n<?php\nB;\n";
        let f = parse(src);
        assert_eq!(f.files.len(), 2);
        assert_eq!(f.files[0].path, "a.php");
        assert_eq!(f.files[0].text, "<?php\nA;\n");
        assert_eq!(f.files[1].path, "b.php");
        assert_eq!(f.files[1].text, "<?php\nB;\n");
    }

    #[test]
    fn cursor_marker_extracted() {
        let src = "//- /x.php\n<?php\n$g->hel$0lo();\n";
        let f = parse(src);
        let c = f.cursor.expect("cursor");
        assert_eq!(c.path, "x.php");
        assert_eq!(c.line, 1);
        assert_eq!(c.character, 7);
        assert_eq!(f.files[0].text, "<?php\n$g->hello();\n");
    }

    #[test]
    fn annotation_default_severity_is_error() {
        let src = "<?php\nfoo();\n// ^^^ is not defined\n";
        let f = parse(src);
        assert_eq!(f.files[0].text, "<?php\nfoo();\n");
        assert_eq!(f.files[0].annotations.len(), 1);
        let a = &f.files[0].annotations[0];
        assert_eq!(a.line, 1);
        assert_eq!(a.start_char, 3);
        assert_eq!(a.end_char, 6);
        assert_eq!(a.severity, "error");
        assert_eq!(a.message, "is not defined");
    }

    #[test]
    fn two_cursors_yield_range() {
        let src = "<?php\n$result = $01 + 2$0;\n";
        let f = parse(src);
        let r = f.range.expect("range");
        assert_eq!(r.start_line, 1);
        assert_eq!(r.start_character, 10);
        assert_eq!(r.end_line, 1);
        assert_eq!(r.end_character, 15);
        assert_eq!(f.files[0].text, "<?php\n$result = 1 + 2;\n");
        // Cursor also exposed at selection start.
        let c = f.cursor.expect("cursor mirrors range start");
        assert_eq!((c.line, c.character), (1, 10));
    }

    #[test]
    fn annotation_explicit_severity() {
        let src = "<?php\nfoo();\n// ^^^ warning: might be slow\n";
        let f = parse(src);
        let a = &f.files[0].annotations[0];
        assert_eq!(a.severity, "warning");
        assert_eq!(a.message, "might be slow");
    }
}
