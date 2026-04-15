use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
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
        group.bench_with_input(BenchmarkId::from_parameter(name), source, |b, src| {
            b.iter(|| {
                let store = DocumentStore::new();
                let uri = Url::parse("file:///bench/file.php").unwrap();
                store.index(uri, src);
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
        b.iter(|| {
            let _ = store.get_doc(&uri);
        });
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
        b.iter(|| store.all_docs());
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
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{n}_files")),
            &n,
            |b, &n| {
                b.iter(|| {
                    let store = DocumentStore::new();
                    for i in 0..n {
                        let (_, src) = fixtures[i % fixtures.len()];
                        let uri = Url::parse(&format!("file:///bench/scan_{i}.php")).unwrap();
                        store.index(uri, src);
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_index_single,
    bench_get_doc,
    bench_all_docs,
    bench_workspace_scan
);
criterion_main!(benches);
