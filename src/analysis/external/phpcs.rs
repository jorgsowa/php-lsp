//! Runs PHPCS against a single file via `--report=json` and maps its
//! findings onto LSP diagnostics.
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use serde::Deserialize;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range};

use crate::lang::config::PhpcsConfig;

pub const SOURCE: &str = "phpcs";

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
    source: Option<String>,
    #[serde(rename = "type")]
    kind: String,
    line: Option<u32>,
    column: Option<u32>,
}

/// Check `path` with PHPCS and return its findings as diagnostics.
///
/// Returns an empty `Vec` whenever the tool can't be run at all (binary
/// missing, spawn failure, unparseable output) — same best-effort contract
/// as [`super::phpstan::run`].
pub async fn run(cfg: &PhpcsConfig, path: &Path, workspace_root: Option<&Path>) -> Vec<Diagnostic> {
    let mut command = tokio::process::Command::new(&cfg.bin_path);
    command.arg("-q").arg("--report=json");
    if let Some(standard) = &cfg.standard {
        command.arg(format!("--standard={standard}"));
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
            tracing::debug!("phpcs ({}) did not run: {e}", cfg.bin_path);
            return Vec::new();
        }
    };

    // PHPCS exits non-zero when it finds violations — expected, unrelated to
    // whether stdout holds a valid report.
    let Ok(report) = serde_json::from_slice::<Report>(&output.stdout) else {
        return Vec::new();
    };

    // Only one file was ever passed on the command line; take the single
    // entry regardless of its key rather than matching by path string (see
    // the same note in `phpstan::run`).
    let Some(file_report) = report.files.into_values().next() else {
        return Vec::new();
    };

    file_report.messages.iter().map(to_diagnostic).collect()
}

fn to_diagnostic(message: &Message) -> Diagnostic {
    let line = message.line.unwrap_or(1).saturating_sub(1);
    let character = message.column.unwrap_or(1).saturating_sub(1);
    Diagnostic {
        range: Range {
            start: Position { line, character },
            end: Position {
                line,
                character: character + 1,
            },
        },
        severity: Some(if message.kind == "WARNING" {
            DiagnosticSeverity::WARNING
        } else {
            DiagnosticSeverity::ERROR
        }),
        code: message.source.clone().map(NumberOrString::String),
        source: Some(SOURCE.to_string()),
        message: message.message.clone(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_shaped_report_and_maps_position_to_zero_based() {
        let json = r#"{
            "totals": {"errors": 1, "warnings": 1, "fixable": 1},
            "files": {
                "/abs/path/Foo.php": {
                    "errors": 1,
                    "warnings": 1,
                    "messages": [
                        {
                            "message": "Line indented incorrectly.",
                            "source": "Generic.WhiteSpace.ScopeIndent.Incorrect",
                            "severity": 5,
                            "fixable": true,
                            "type": "ERROR",
                            "line": 12,
                            "column": 5
                        },
                        {
                            "message": "Line exceeds 120 characters.",
                            "source": "Generic.Files.LineLength.TooLong",
                            "severity": 5,
                            "fixable": false,
                            "type": "WARNING",
                            "line": 20,
                            "column": 1
                        }
                    ]
                }
            }
        }"#;
        let report: Report = serde_json::from_str(json).unwrap();
        let file_report = report.files.into_values().next().unwrap();
        let diagnostics: Vec<Diagnostic> = file_report.messages.iter().map(to_diagnostic).collect();

        assert_eq!(diagnostics.len(), 2);

        let error = &diagnostics[0];
        assert_eq!(
            error.range.start,
            Position {
                line: 11,
                character: 4
            }
        );
        assert_eq!(error.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(
            error.code,
            Some(NumberOrString::String(
                "Generic.WhiteSpace.ScopeIndent.Incorrect".to_string()
            ))
        );

        let warning = &diagnostics[1];
        assert_eq!(warning.severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn missing_position_falls_back_to_the_first_character() {
        let message = Message {
            message: "Some violation".to_string(),
            source: None,
            kind: "ERROR".to_string(),
            line: None,
            column: None,
        };
        let d = to_diagnostic(&message);
        assert_eq!(
            d.range,
            Range {
                start: Position {
                    line: 0,
                    character: 0
                },
                end: Position {
                    line: 0,
                    character: 1
                },
            }
        );
    }
}
