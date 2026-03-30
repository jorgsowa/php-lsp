//! `mir-php` CLI — run static analysis on PHP files.
//!
//! Usage:
//!   mir-php [--json] <file.php> [<file.php> ...]
//!
//! Without `--json` prints human-readable diagnostics to stdout.
//! With `--json` prints a JSON array of diagnostic objects.
//!
//! Exit code is 0 when no warnings or errors are found, 1 otherwise.
use std::env;
use std::fs;
use std::process;
use std::sync::Arc;

use bumpalo::Bump;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let (json_mode, files) = if args.first().map(|s| s.as_str()) == Some("--json") {
        (true, &args[1..])
    } else {
        (false, &args[..])
    };

    if files.is_empty() {
        eprintln!("Usage: mir-php [--json] <file.php> [...]");
        process::exit(2);
    }

    // Parse all files first so cross-file definitions are available.
    let mut parsed: Vec<(String, Arc<Bump>, Vec<u8>)> = Vec::new();
    for path in files {
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("mir-php: cannot read {path}: {e}");
                process::exit(2);
            }
        };
        let arena = Arc::new(Bump::new());
        parsed.push((source, arena, path.as_bytes().to_vec()));
    }

    // We need to keep arenas alive while programs are referenced.
    // Parse in two passes: first build source+arena pairs, then run analysis.
    let mut sources_and_arenas: Vec<(String, Bump)> = parsed
        .into_iter()
        .map(|(src, _, _)| (src, Bump::new()))
        .collect();

    // Store programs separately after parsing (lifetimes tied to arenas above).
    let mut all_diagnostics: Vec<(String, Vec<mir_php::Diagnostic>)> = Vec::new();
    let mut had_issues = false;

    // We can't easily hold references across iterations due to lifetimes, so
    // run per-file analysis independently (cross-file def lookup is a future enhancement).
    for (i, path) in files.iter().enumerate() {
        let (ref source, ref arena) = sources_and_arenas[i];
        let result = php_rs_parser::parse(arena, source);
        let program = result.program;
        let diags = mir_php::analyze(source, &program.stmts, &[(source.as_str(), &program.stmts)]);
        if !diags.is_empty() {
            had_issues = true;
        }
        all_diagnostics.push((path.clone(), diags));
    }

    if json_mode {
        // Emit a flat JSON array of {file, ...diagnostic} objects.
        #[derive(serde::Serialize)]
        struct Entry<'a> {
            file: &'a str,
            #[serde(flatten)]
            diag: &'a mir_php::Diagnostic,
        }
        let entries: Vec<Entry<'_>> = all_diagnostics
            .iter()
            .flat_map(|(file, diags)| {
                diags.iter().map(move |d| Entry { file, diag: d })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else {
        for (file, diags) in &all_diagnostics {
            for d in diags {
                let sev = match d.severity {
                    mir_php::Severity::Error => "error",
                    mir_php::Severity::Warning => "warning",
                    mir_php::Severity::Information => "info",
                    mir_php::Severity::Hint => "hint",
                };
                println!(
                    "{}:{}:{}: {}: {}",
                    file,
                    d.start_line + 1,
                    d.start_char + 1,
                    sev,
                    d.message
                );
            }
        }
    }

    process::exit(if had_issues { 1 } else { 0 });
}
