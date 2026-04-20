/// Standalone memory benchmark: index a directory of PHP files and report
/// peak + final RSS.  Run with `--features dhat-heap` for heap profiling.
///
/// Two modes (controlled by `--full` flag):
///
///   # FileIndex only (DocumentStore, fast):
///   cargo run --release --bin mem_index -- benches/fixtures/laravel/src
///
///   # Full pipeline (DocumentStore + Codebase + finalize, matches real LSP):
///   cargo run --release --bin mem_index -- --full benches/fixtures/laravel/src
///
///   # Full pipeline with heap profile:
///   cargo run --release --features dhat-heap --bin mem_index -- --full benches/fixtures/laravel/src

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use std::sync::Arc;
use std::time::Instant;

use tower_lsp::lsp_types::Url;

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
            "  --full   also run DefinitionCollector + codebase.finalize() (full LSP pipeline)"
        );
        std::process::exit(1);
    });

    let dir = std::fs::canonicalize(&dir).unwrap_or_else(|_| {
        eprintln!("error: directory not found: {dir}");
        std::process::exit(1);
    });

    let php_files: Vec<(Url, String)> = walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "php"))
        .filter_map(|e| {
            let url = Url::from_file_path(e.path()).ok()?;
            let src = std::fs::read_to_string(e.path()).ok()?;
            Some((url, src))
        })
        .collect();

    println!("Files found:    {}", php_files.len());
    println!(
        "Mode:           {}",
        if full_pipeline {
            "full (DocumentStore + Codebase)"
        } else {
            "index-only (DocumentStore)"
        }
    );
    println!();

    let rss_before = rss_kb();
    let t0 = Instant::now();

    let store = DocumentStore::new();
    let codebase = if full_pipeline {
        Some(Arc::new(mir_codebase::Codebase::new()))
    } else {
        None
    };

    let mut peak_rss = rss_before;

    for (i, (url, src)) in php_files.iter().enumerate() {
        if let Some(cb) = &codebase {
            // Replicate the real scan_workspace pipeline:
            // 1. Parse once to get AST
            // 2. Run DefinitionCollector into codebase
            // 3. Store FileIndex reusing the same ParsedDoc (no second parse)
            let doc = ParsedDoc::parse(src.clone());
            let file: Arc<str> = Arc::from(url.as_str());
            let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());
            let collector = mir_analyzer::collector::DefinitionCollector::new(
                cb,
                file,
                doc.source(),
                &source_map,
            );
            collector.collect(doc.program());
            store.index_from_doc(url.clone(), &doc, vec![]);
        } else {
            store.index(url.clone(), src);
        }

        if i % 100 == 0 {
            let rss = rss_kb();
            if rss > peak_rss {
                peak_rss = rss;
            }
        }
    }

    let rss_after_index = rss_kb();
    if rss_after_index > peak_rss {
        peak_rss = rss_after_index;
    }

    // Finalize the codebase (rebuilds full inheritance tables) — this is what
    // makes the first did_change after workspace scan expensive.
    if let Some(cb) = &codebase {
        let t_fin = Instant::now();
        cb.finalize();
        println!(
            "codebase.finalize(): {:.1}ms",
            t_fin.elapsed().as_secs_f64() * 1000.0
        );
        let rss_fin = rss_kb();
        if rss_fin > peak_rss {
            peak_rss = rss_fin;
        }
    }

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
    if codebase.is_some() {
        print_rss("RSS after finalize", rss_final);
    }
    print_rss("RSS peak (sampled)", peak_rss);
    println!();
    let delta = peak_rss.saturating_sub(rss_before);
    print_rss("Delta (peak - before)", delta);
    if let Some(post) = rss_after_index.checked_sub(rss_before) {
        print_rss("  DocumentStore share", post);
    }
    if codebase.is_some()
        && let Some(cb_share) = rss_final.checked_sub(rss_after_index)
    {
        print_rss("  Codebase share", cb_share);
    }
}
