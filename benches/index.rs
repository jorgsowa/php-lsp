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
        let uri = Url::parse(&format!("file:///bench/file{}.php", i)).unwrap();
        store.index(uri, SMALL);
    }

    c.bench_function("index/all_docs_10", |b| {
        b.iter(|| store.all_docs());
    });
}

criterion_group!(benches, bench_index_single, bench_get_doc, bench_all_docs);
criterion_main!(benches);
