//! Runs PHPStan against a single file via `--error-format=json` and maps its
//! findings onto LSP diagnostics.
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use serde::Deserialize;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

use crate::lang::config::PhpstanConfig;

pub const SOURCE: &str = "phpstan";

#[derive(Debug, Deserialize)]
struct Report {
    #[serde(default)]
    files: HashMap<String, FileReport>,
}

#[derive(Debug, Deserialize)]
struct FileReport {
    #[serde(default)]
    messages: Vec<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    message: String,
    /// Absent for a handful of file-level errors (e.g. a parse failure
    /// PHPStan itself can't attribute to a line); those fall back to line 1.
    line: Option<u32>,
    /// Rule identifier (PHPStan >= 1.11, e.g. `"argument.type"`). Older
    /// PHPStan versions omit it.
    identifier: Option<String>,
}

/// Analyse `path` with PHPStan and return its findings as diagnostics.
///
/// PHPStan reports only a line number, never a column, so each diagnostic
/// spans the full line (`character: u32::MAX` on the end position — editors
/// clamp this to the actual line length, the same convention other
/// line-only-precision tools use rather than re-reading the source here).
///
/// Returns an empty `Vec` whenever the tool can't be run at all (binary
/// missing, spawn failure, unparseable output) — this is an opt-in, best-
/// effort overlay on top of the built-in diagnostics, not a required source
/// of truth, so a broken PHPStan setup degrades silently rather than
/// erroring the whole diagnostics publish.
pub async fn run(
    cfg: &PhpstanConfig,
    path: &Path,
    workspace_root: Option<&Path>,
) -> Vec<Diagnostic> {
    let mut command = tokio::process::Command::new(&cfg.bin_path);
    command
        .arg("analyse")
        .arg("--error-format=json")
        .arg("--no-progress")
        .arg("--no-interaction")
        .arg("--memory-limit=1G");
    if let Some(config_path) = &cfg.config_path {
        command.arg("-c").arg(config_path);
    }
    command.arg(path);
    if let Some(root) = workspace_root {
        command.current_dir(root);
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());

    let output = match command.output().await {
        Ok(o) => o,
        Err(e) => {
            tracing::debug!("phpstan ({}) did not run: {e}", cfg.bin_path);
            return Vec::new();
        }
    };

    // PHPStan exits non-zero when it finds errors — that's expected and
    // unrelated to whether stdout holds a valid report, so the exit code is
    // deliberately not checked here.
    let Ok(report) = serde_json::from_slice::<Report>(&output.stdout) else {
        return Vec::new();
    };

    // Only one file was ever passed on the command line, so take whatever
    // single entry came back rather than matching by path string — PHPStan
    // may report a canonicalized (symlink-resolved) path that differs
    // byte-for-byte from the one we passed.
    let Some(file_report) = report.files.into_values().next() else {
        return Vec::new();
    };

    file_report.messages.iter().map(to_diagnostic).collect()
}

fn to_diagnostic(message: &Message) -> Diagnostic {
    let line = message.line.unwrap_or(1).saturating_sub(1);
    Diagnostic {
        range: Range {
            start: Position { line, character: 0 },
            end: Position {
                line,
                character: u32::MAX,
            },
        },
        severity: Some(DiagnosticSeverity::ERROR),
        code: message.identifier.clone().map(NumberOrString::String),
        source: Some(SOURCE.to_string()),
        message: message.message.clone(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_report_and_maps_line_to_zero_based() {
        let json = r#"{
            "totals": {"errors": 1, "file_errors": 1},
            "files": {
                "/abs/path/Foo.php": {
                    "errors": 1,
                    "messages": [
                        {
                            "message": "Parameter #1 $x of method Foo::bar() expects int, string given.",
                            "line": 10,
                            "ignorable": true,
                            "identifier": "argument.type"
                        }
                    ]
                }
            },
            "errors": []
        }"#;
        let report: Report = serde_json::from_str(json).unwrap();
        let file_report = report.files.into_values().next().unwrap();
        let diagnostics: Vec<Diagnostic> = file_report.messages.iter().map(to_diagnostic).collect();

        assert_eq!(diagnostics.len(), 1);
        let d = &diagnostics[0];
        assert_eq!(
            d.range.start,
            Position {
                line: 9,
                character: 0
            }
        );
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(d.source.as_deref(), Some("phpstan"));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("argument.type".to_string()))
        );
        assert!(d.message.contains("expects int, string given"));
    }

    #[test]
    fn missing_line_falls_back_to_the_first_line() {
        let message = Message {
            message: "Internal error".to_string(),
            line: None,
            identifier: None,
        };
        let d = to_diagnostic(&message);
        assert_eq!(
            d.range.start,
            Position {
                line: 0,
                character: 0
            }
        );
        assert_eq!(d.code, None);
    }

    #[test]
    fn empty_files_map_yields_no_diagnostics() {
        let json = r#"{"files": {}, "errors": []}"#;
        let report: Report = serde_json::from_str(json).unwrap();
        assert!(report.files.into_values().next().is_none());
    }
}
