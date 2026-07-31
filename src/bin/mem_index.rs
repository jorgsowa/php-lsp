/// Standalone memory benchmark: index a directory of PHP files and report
/// peak + final RSS.  Run with `--features dhat-heap` for heap profiling.
///
/// Two modes (controlled by `--full` flag):
///
///   # FileIndex only (DocumentStore, fast):
///   cargo run --release --bin mem_index -- benches/fixtures/laravel/src
///
///   # Full pipeline (DocumentStore + Codebase, matches real LSP):
///   cargo run --release --bin mem_index -- --full benches/fixtures/laravel/src
///
///   # Full pipeline with heap profile:
///   cargo run --release --features dhat-heap --bin mem_index -- --full benches/fixtures/laravel/src
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use tower_lsp_server::ls_types::Uri;

use php_lsp::ast::ParsedDoc;
use php_lsp::document_store::DocumentStore;

fn rss_kb() -> u64 {
    // macOS
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let pid = std::process::id();
        if let Ok(out) = Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            && let Ok(s) = std::str::from_utf8(&out.stdout)
            && let Ok(n) = s.trim().parse::<u64>()
        {
            return n;
        }
    }
    // Linux
    #[cfg(target_os = "linux")]
    if let Ok(s) = std::fs::read_to_string(format!("/proc/{}/status", std::process::id())) {
        for line in s.lines() {
            if line.starts_with("VmRSS:")
                && let Some(n) = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
            {
                return n;
            }
        }
    }
    0
}

fn print_rss(label: &str, kb: u64) {
    println!(
        "{:<24} {} KB ({:.1} MB)",
        format!("{label}:"),
        kb,
        kb as f64 / 1024.0
    );
}

fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    // Parse args: optional `--full` flag before the directory path.
    let args: Vec<String> = std::env::args().collect();
    let (full_pipeline, dir_arg) = match args.get(1).map(|s| s.as_str()) {
        Some("--full") => (true, args.get(2)),
        _ => (false, args.get(1)),
    };

    let dir = dir_arg.cloned().unwrap_or_else(|| {
        eprintln!("Usage: mem_index [--full] <directory>");
        eprintln!(
            "  --full   also run DefinitionCollector + codebase resolution (full LSP pipeline)"
        );
        std::process::exit(1);
    });

    let dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| {
        eprintln!("error: directory not found: {dir}");
        std::process::exit(1);
    });

    let php_paths: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "php"))
        .map(|e| e.path().to_path_buf())
        .collect();

    let php_files: Vec<(Uri, String)> = php_paths
        .par_iter()
        .filter_map(|p| {
            let url = Uri::from_file_path(p)?;
            let src = std::fs::read_to_string(p).ok()?;
            Some((url, src))
        })
        .collect();

    println!("Files found:    {}", php_files.len());
    println!(
        "Mode:           {}",
        if full_pipeline {
            "full (DocumentStore + MirDb)"
        } else {
            "index-only (DocumentStore)"
        }
    );
    println!();

    let rss_before = rss_kb();
    let t0 = Instant::now();

    let store = DocumentStore::new();
    let session = if full_pipeline {
        Some(mir_analyzer::AnalysisSession::new(
            mir_analyzer::PhpVersion::LATEST,
        ))
    } else {
        None
    };

    for (url, src) in php_files.iter() {
        if let Some(s) = session.as_ref() {
            let src_arc: Arc<str> = Arc::from(src.as_str());
            let doc = ParsedDoc::parse(src_arc.clone());
            let file: Arc<str> = Arc::from(url.as_str());
            s.ingest_file(file, src_arc);
            store.ingest_from_doc(url.clone(), &doc);
        } else {
            store.ingest(url.clone(), src);
        }
    }

    // Sample RSS once after the loop. The previous in-loop polling via
    // `Command::new("ps")` cost ~17% of total profile time on Laravel.
    let rss_after_index = rss_kb();
    let peak_rss = rss_after_index.max(rss_before);

    let elapsed = t0.elapsed();
    let rss_final = rss_kb();
    let _indexes = store.all_indexes(); // force retention

    println!(
        "Indexed {} files in {:.1}s",
        php_files.len(),
        elapsed.as_secs_f64()
    );
    println!();
    print_rss("RSS before", rss_before);
    print_rss("RSS after index", rss_after_index);
    print_rss("RSS peak (sampled)", peak_rss);
    println!();
    let delta = peak_rss.saturating_sub(rss_before);
    print_rss("Delta (peak - before)", delta);
    if let Some(post) = rss_after_index.checked_sub(rss_before) {
        print_rss("  DocumentStore share", post);
    }
    let _ = (session, rss_final);
}
