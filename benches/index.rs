use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
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
                store.ingest(uri.clone(), src);
            });
        });
    }
    group.finish();
}

/// Benchmark retrieving a parsed doc after indexing.
fn bench_get_doc(c: &mut Criterion) {
    let store = DocumentStore::new();
    let uri = Url::parse("file:///bench/medium.php").unwrap();
    store.ingest(uri.clone(), MEDIUM);

    c.bench_function("index/get_doc", |b| {
        b.iter(|| black_box(store.get_doc_salsa(&uri)));
    });
}

/// Benchmark resolving 10 open-file URLs to parsed docs via `docs_for`.
fn bench_all_docs(c: &mut Criterion) {
    let store = DocumentStore::new();
    let urls: Vec<Url> = (0..10)
        .map(|i| Url::parse(&format!("file:///bench/file{i}.php")).unwrap())
        .collect();
    for u in &urls {
        store.ingest(u.clone(), SMALL);
    }

    c.bench_function("index/all_docs_10", |b| {
        b.iter(|| black_box(store.docs_for(&urls)));
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
                        store.ingest(uris[i].clone(), src);
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
        .filter(|e| e.path().extension().is_some_and(|x| x == "php"))
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
                store.ingest(url.clone(), src);
            }
        });
    });

    group.finish();
}

/// Phase G2 contention micro-bench: N threads concurrently re-mirror the
/// same URL with the same text. Single-threaded this work is cheap either
/// way, but with the fast path disabled every thread serialises on
/// `host.lock()` just to confirm a no-op; the G2 cache hit lets them
/// proceed in parallel. Mirrors what happens during workspace scan +
/// `did_open` on already-indexed files under a multi-core tokio runtime.
fn bench_mirror_same_text_contended(c: &mut Criterion) {
    use std::sync::Arc;

    let store = Arc::new(DocumentStore::new());
    let uri = Url::parse("file:///bench/mirror.php").unwrap();
    store.ingest(uri.clone(), MEDIUM);

    let threads = 8usize;
    let iters_per_thread = 500usize;

    c.bench_function("index/mirror_same_text_contended_8x500", |b| {
        b.iter(|| {
            let mut handles = Vec::with_capacity(threads);
            for _ in 0..threads {
                let store = Arc::clone(&store);
                let uri = uri.clone();
                handles.push(std::thread::spawn(move || {
                    for _ in 0..iters_per_thread {
                        store.ingest(black_box(uri.clone()), black_box(MEDIUM));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
        });
    });
}

/// Measures `get_doc_salsa` on an already-parsed file — the hot path for
/// every feature-module call in `backend.rs`. With G3 enabled this should
/// hit the lock-free `parsed_cache`; without G3 every call goes through
/// `snapshot_query`. Contrast the two to decide whether G3 is worth keeping.
fn bench_get_doc_repeated(c: &mut Criterion) {
    let store = DocumentStore::new();
    let uri = Url::parse("file:///bench/hotdoc.php").unwrap();
    store.ingest(uri.clone(), MEDIUM);
    let _warm = store.get_doc_salsa(&uri);

    c.bench_function("index/get_doc_repeated", |b| {
        b.iter(|| black_box(store.get_doc_salsa(black_box(&uri))));
    });
}

/// Benchmark `sync_workspace_files` on the **clean path** (dirty flag clear).
///
/// Models the common case after workspace scan: no files added/removed.
/// This is what every hover/symbol/hierarchy request pays per call to
/// `get_workspace_index_salsa`. Should be O(1) — just an atomic swap.
fn bench_sync_workspace_clean(c: &mut Criterion) {
    let fixtures: &[(&str, &str)] = &[
        ("small_class", SMALL),
        ("medium_class", MEDIUM),
        ("interface_large", LARGE_IFACE),
    ];

    let mut group = c.benchmark_group("index/sync_workspace_files/clean");

    for &n in &[10usize, 100, 500] {
        let store = DocumentStore::new();
        let uris: Vec<Url> = (0..n)
            .map(|i| Url::parse(&format!("file:///bench/clean_{i}.php")).unwrap())
            .collect();
        for (i, uri) in uris.iter().enumerate() {
            let (_, src) = fixtures[i % fixtures.len()];
            store.ingest(uri.clone(), src);
        }
        // Warm: initial sync so the dirty flag is clear.
        let _ = store.get_workspace_index_salsa();

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{n}_files")),
            &n,
            |b, _| {
                b.iter(|| {
                    store.sync_workspace_files();
                    black_box(())
                });
            },
        );
    }
    group.finish();
}

/// Benchmark `sync_workspace_files` on the **dirty path** (file set unchanged
/// but flag set) — models O(N) collect-under-one-lock vs. the old O(N log N)
/// lock-per-comparison. Before the fix this path ran on *every* call.
fn bench_sync_workspace_dirty(c: &mut Criterion) {
    let fixtures: &[(&str, &str)] = &[
        ("small_class", SMALL),
        ("medium_class", MEDIUM),
        ("interface_large", LARGE_IFACE),
    ];

    let mut group = c.benchmark_group("index/sync_workspace_files/dirty");

    for &n in &[10usize, 100, 500] {
        let store = DocumentStore::new();
        let uris: Vec<Url> = (0..n)
            .map(|i| Url::parse(&format!("file:///bench/dirty_{i}.php")).unwrap())
            .collect();
        for (i, uri) in uris.iter().enumerate() {
            let (_, src) = fixtures[i % fixtures.len()];
            store.ingest(uri.clone(), src);
        }
        // Warm: initial sync so workspace salsa input is set.
        let _ = store.get_workspace_index_salsa();

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{n}_files")),
            &n,
            |b, _| {
                b.iter(|| {
                    // Reset the dirty flag before each call to simulate the
                    // old behaviour where sync always ran the full path.
                    store.mark_workspace_files_dirty();
                    store.sync_workspace_files();
                    black_box(());
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
    bench_workspace_scan,
    bench_workspace_scan_laravel,
    bench_mirror_same_text_contended,
    bench_get_doc_repeated,
    bench_sync_workspace_clean,
    bench_sync_workspace_dirty,
);
criterion_main!(benches);
