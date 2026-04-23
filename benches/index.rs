use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use tower_lsp::lsp_types::Url;

use php_lsp::document_store::DocumentStore;

const SMALL: &str = include_str!("fixtures/small_class.php");
const MEDIUM: &str = include_str!("fixtures/medium_class.php");
const LARGE_IFACE: &str = include_str!("fixtures/interface_large.php");

/// Benchmark inserting a single file via `DocumentStore::index`.
fn bench_index_single(c: &mut Criterion) {
    let mut group = c.benchmark_group("index/single");

    for (name, source) in [
        ("small_class", SMALL),
        ("medium_class", MEDIUM),
        ("interface_large", LARGE_IFACE),
    ] {
        let uri = Url::parse("file:///bench/file.php").unwrap();
        group.bench_with_input(BenchmarkId::from_parameter(name), source, |b, src| {
            b.iter(|| {
                let store = DocumentStore::new();
                store.index(uri.clone(), src);
            });
        });
    }
    group.finish();
}

/// Benchmark retrieving a parsed doc after indexing.
fn bench_get_doc(c: &mut Criterion) {
    let store = DocumentStore::new();
    let uri = Url::parse("file:///bench/medium.php").unwrap();
    store.index(uri.clone(), MEDIUM);

    c.bench_function("index/get_doc", |b| {
        b.iter(|| black_box(store.get_doc_salsa(&uri)));
    });
}

/// Benchmark `all_docs` with 10 indexed files.
fn bench_all_docs(c: &mut Criterion) {
    let store = DocumentStore::new();
    for i in 0..10 {
        let uri = Url::parse(&format!("file:///bench/file{i}.php")).unwrap();
        store.index(uri, SMALL);
    }

    c.bench_function("index/all_docs_10", |b| {
        b.iter(|| black_box(store.all_docs()));
    });
}

/// Benchmark a simulated workspace scan: index N files sequentially into a fresh store.
/// Models "workspace indexing time" from the issue — how long it takes to build an index
/// from scratch for a codebase of a given size.
fn bench_workspace_scan(c: &mut Criterion) {
    // Round-robin across the three fixture files so the content is realistic.
    let fixtures: &[(&str, &str)] = &[
        ("small_class", SMALL),
        ("medium_class", MEDIUM),
        ("interface_large", LARGE_IFACE),
    ];

    let mut group = c.benchmark_group("index/workspace_scan");

    for &n in &[1usize, 10, 50] {
        // Pre-generate URIs so URL parsing doesn't inflate the measurement.
        let uris: Vec<Url> = (0..n)
            .map(|i| Url::parse(&format!("file:///bench/scan_{i}.php")).unwrap())
            .collect();

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{n}_files")),
            &n,
            |b, &n| {
                b.iter(|| {
                    let store = DocumentStore::new();
                    for i in 0..n {
                        let (_, src) = fixtures[i % fixtures.len()];
                        store.index(uris[i].clone(), src);
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark indexing the Laravel framework (~2,500 PHP files).
///
/// Requires running `scripts/setup_laravel_fixture.sh` first.
/// Skipped automatically if the fixture is absent.
fn bench_workspace_scan_laravel(c: &mut Criterion) {
    let fixture_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures/laravel/src");

    if !fixture_dir.exists() {
        eprintln!(
            "Laravel fixture not found — run `scripts/setup_laravel_fixture.sh` to enable this benchmark"
        );
        return;
    }

    let php_files: Vec<(tower_lsp::lsp_types::Url, String)> = walkdir::WalkDir::new(&fixture_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |x| x == "php"))
        .filter_map(|e| {
            let url = tower_lsp::lsp_types::Url::from_file_path(e.path()).ok()?;
            let src = std::fs::read_to_string(e.path()).ok()?;
            Some((url, src))
        })
        .collect();

    eprintln!("Laravel fixture: {} PHP files", php_files.len());

    let mut group = c.benchmark_group("index/workspace_scan");
    group.sample_size(10);

    group.bench_function("laravel_framework", |b| {
        b.iter(|| {
            let store = DocumentStore::new();
            // Phase F: DocumentStore no longer has a hand-written LRU, so
            // there is no eviction to disable; `index()` unconditionally
            // keeps every file in the mirror. The old `set_max_indexed`
            // call has been removed.
            for (url, src) in &php_files {
                store.index(url.clone(), src);
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_index_single,
    bench_get_doc,
    bench_all_docs,
    bench_workspace_scan,
    bench_workspace_scan_laravel
);
criterion_main!(benches);
