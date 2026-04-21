//! mir-analyzer benchmarks: measures the static-analysis pipeline
//! (DefinitionCollector → finalize → StatementsAnalyzer) that powers
//! semantic diagnostics.
//!
//! Covers three regimes:
//!   - `single_file`      — fresh codebase per iter (cold analyze cost)
//!   - `edit_loop`        — persistent codebase, repeated analyze on the same
//!                          file (models per-keystroke re-analyze cost on a
//!                          small workspace)
//!   - `laravel_scale`    — full Laravel populated into the codebase once,
//!                          then one representative file re-analyzed (models
//!                          per-keystroke cost in a realistic large workspace)
//!
//! The Laravel-scale bench is auto-skipped when the fixture is absent.
//! Run `scripts/setup_laravel_fixture.sh` to enable it.

use std::sync::Arc;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use tower_lsp::lsp_types::Url;

use php_lsp::ast::ParsedDoc;
use php_lsp::backend::DiagnosticsConfig;
use php_lsp::semantic_diagnostics::{semantic_diagnostics, semantic_diagnostics_no_rebuild};

const MEDIUM: &str = include_str!("fixtures/medium_class.php");

fn all_enabled() -> DiagnosticsConfig {
    // Constructed literally because `DiagnosticsConfig::all_enabled()` is
    // `#[cfg(test)]` and not visible to benches.
    DiagnosticsConfig {
        enabled: true,
        undefined_variables: true,
        undefined_functions: true,
        undefined_classes: true,
        arity_errors: true,
        type_errors: true,
        deprecated_calls: true,
        duplicate_declarations: true,
    }
}

/// Single-file cold analyze: fresh `Codebase` on every iteration.
fn bench_single_file(c: &mut Criterion) {
    let uri = Url::parse("file:///bench/medium.php").unwrap();
    let doc = ParsedDoc::parse(MEDIUM.to_owned());
    let cfg = all_enabled();

    c.bench_function("semantic/single_file/medium", |b| {
        b.iter(|| {
            let codebase = mir_codebase::Codebase::new();
            black_box(semantic_diagnostics(&uri, &doc, &codebase, &cfg, None));
        });
    });
}

/// Edit-loop: codebase is re-populated in place per iter via
/// `semantic_diagnostics` (which evicts the file's definitions, re-collects,
/// re-finalizes, then analyzes). Models the per-keystroke cost on a
/// small workspace where the changed file is the only thing in the codebase.
fn bench_edit_loop(c: &mut Criterion) {
    let uri = Url::parse("file:///bench/medium.php").unwrap();
    let doc = ParsedDoc::parse(MEDIUM.to_owned());
    let cfg = all_enabled();
    let codebase = mir_codebase::Codebase::new();

    // Warm the codebase so the first iter isn't an outlier.
    let _ = semantic_diagnostics(&uri, &doc, &codebase, &cfg, None);

    c.bench_function("semantic/edit_loop/medium", |b| {
        b.iter(|| {
            black_box(semantic_diagnostics(&uri, &doc, &codebase, &cfg, None));
        });
    });
}

/// Laravel-scale edit-loop: populate the codebase with all Laravel definitions
/// once, finalize once, then on each iter re-analyze one representative file
/// **without** rebuilding the codebase. Measures per-keystroke re-analyze cost
/// in a realistic large workspace.
fn bench_laravel_scale(c: &mut Criterion) {
    let fixture_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel/src");

    if !fixture_dir.exists() {
        eprintln!(
            "Laravel fixture not found — run `scripts/setup_laravel_fixture.sh` to enable semantic/laravel_scale"
        );
        return;
    }

    // Load + parse every PHP file up-front (not counted in the bench).
    let parsed: Vec<(Url, Arc<ParsedDoc>, String)> = walkdir::WalkDir::new(&fixture_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |x| x == "php"))
        .filter_map(|e| {
            let url = Url::from_file_path(e.path()).ok()?;
            let src = std::fs::read_to_string(e.path()).ok()?;
            let doc = Arc::new(ParsedDoc::parse(src.clone()));
            Some((url, doc, src))
        })
        .collect();

    eprintln!("Laravel fixture: {} PHP files (semantic)", parsed.len());

    // Populate codebase with every file's definitions, then finalize once.
    let codebase = mir_codebase::Codebase::new();
    for (url, doc, _) in &parsed {
        let file: Arc<str> = Arc::from(url.as_str());
        let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());
        let collector = mir_analyzer::collector::DefinitionCollector::new(
            &codebase,
            file,
            doc.source(),
            &source_map,
        );
        collector.collect(doc.program());
    }
    codebase.finalize();

    // Pick a representative hot file to re-analyze on each iter. Prefer a
    // well-known Illuminate file; fall back to the first parsed entry.
    let hot = parsed
        .iter()
        .find(|(u, _, _)| u.as_str().ends_with("/Illuminate/Support/Str.php"))
        .or_else(|| parsed.first())
        .expect("at least one laravel fixture file");

    let cfg = all_enabled();
    let mut group = c.benchmark_group("semantic/laravel_scale");
    group.sample_size(20);

    group.bench_function("reanalyze_str", |b| {
        b.iter(|| {
            black_box(semantic_diagnostics_no_rebuild(
                &hot.0, &hot.1, &codebase, &cfg, None,
            ));
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_single_file,
    bench_edit_loop,
    bench_laravel_scale
);
criterion_main!(benches);
