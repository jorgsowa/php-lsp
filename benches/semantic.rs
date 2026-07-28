//! mir-analyzer benchmarks: measures the session-based analyze pipeline
//! that powers semantic diagnostics.
//!
//! Covers three regimes:
//! - `single_file` — fresh AnalysisSession per iter (cold analyze cost)
//! - `edit_loop` — persistent session, repeated analyze on the same file
//!   (models per-keystroke re-analyze cost on a small workspace)
//! - `laravel_scale` — full Laravel ingested into the session once, then one
//!   representative file re-analyzed (models per-keystroke cost in a realistic
//!   large workspace)
//!
//! The Laravel-scale bench is auto-skipped when the fixture is absent.
//! Run `scripts/setup_laravel_fixture.sh` to enable it.

use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use tower_lsp::lsp_types::Url;

use php_lsp::ast::ParsedDoc;
use php_lsp::config::DiagnosticsConfig;
use php_lsp::semantic_diagnostics::semantic_diagnostics;

const MEDIUM: &str = include_str!("fixtures/medium_class.php");

fn all_enabled() -> DiagnosticsConfig {
    // `DiagnosticsConfig::all_enabled()` is `#[cfg(test)]`; reconstruct
    // here so the bench can build outside the test profile.
    DiagnosticsConfig {
        enabled: true,
        undefined_variables: true,
        undefined_functions: true,
        undefined_classes: true,
        arity_errors: true,
        type_errors: true,
        deprecated_calls: true,
        duplicate_declarations: true,
        unused_symbols: false,
        missing_types: false,
        mixed_usage: false,
    }
}

fn new_session() -> mir_analyzer::AnalysisSession {
    // Sessions here run mir's parallel analysis before any DocumentStore
    // exists — size the rayon stacks like production first.
    php_lsp::document_store::ensure_rayon_worker_stacks();
    let s = mir_analyzer::AnalysisSession::new(mir_analyzer::PhpVersion::LATEST);
    s.ensure_all_stubs();
    s
}

/// Single-file cold analyze: fresh `AnalysisSession` per iteration.
fn bench_single_file(c: &mut Criterion) {
    let uri = Url::parse("file:///bench/medium.php").unwrap();
    let doc = ParsedDoc::parse(MEDIUM.to_owned());
    let cfg = all_enabled();

    c.bench_function("semantic/single_file/medium", |b| {
        b.iter(|| {
            let session = new_session();
            black_box(semantic_diagnostics(&uri, &doc, &session, &cfg));
        });
    });
}

/// Edit-loop: session persists; `ingest_file` updates it in place per iter.
fn bench_edit_loop(c: &mut Criterion) {
    let uri = Url::parse("file:///bench/medium.php").unwrap();
    let doc = ParsedDoc::parse(MEDIUM.to_owned());
    let cfg = all_enabled();
    let session = new_session();

    // Warm so the first iter isn't an outlier.
    let _ = semantic_diagnostics(&uri, &doc, &session, &cfg);

    c.bench_function("semantic/edit_loop/medium", |b| {
        b.iter(|| {
            black_box(semantic_diagnostics(&uri, &doc, &session, &cfg));
        });
    });
}

/// Laravel-scale edit-loop: ingest every file once into a persistent session,
/// then on each iter re-analyze a representative hot file. Measures
/// per-keystroke re-analyze cost in a large workspace.
fn bench_laravel_scale(c: &mut Criterion) {
    let fixture_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel/src");

    if !fixture_dir.exists() {
        eprintln!(
            "Laravel fixture not found — run `scripts/setup_laravel_fixture.sh` to enable semantic/laravel_scale"
        );
        return;
    }

    let parsed: Vec<(Url, Arc<ParsedDoc>, Arc<str>)> = walkdir::WalkDir::new(&fixture_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "php"))
        .filter_map(|e| {
            let url = Url::from_file_path(e.path()).ok()?;
            let src = std::fs::read_to_string(e.path()).ok()?;
            let src_arc: Arc<str> = Arc::from(src);
            let doc = Arc::new(ParsedDoc::parse(src_arc.clone()));
            Some((url, doc, src_arc))
        })
        .collect();

    eprintln!("Laravel fixture: {} PHP files (semantic)", parsed.len());

    let session = new_session();
    for (url, _doc, src_arc) in &parsed {
        let file: Arc<str> = Arc::from(url.as_str());
        session.ingest_file(file, src_arc.clone());
    }

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
            black_box(semantic_diagnostics(&hot.0, &hot.1, &session, &cfg));
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
