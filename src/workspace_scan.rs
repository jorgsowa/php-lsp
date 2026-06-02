use std::sync::Arc;

use rayon::prelude::*;
use tower_lsp::Client;
use tower_lsp::lsp_types::Url;
use tower_lsp::lsp_types::request::{
    CodeLensRefresh, InlayHintRefreshRequest, InlineValueRefreshRequest, SemanticTokensRefresh,
    WorkspaceDiagnosticRefresh,
};

use crate::diagnostics::parse_document_no_diags;
use crate::document_store::DocumentStore;
use crate::open_files::OpenFiles;

/// Ask all connected clients to re-request semantic tokens, code lenses, inlay hints,
/// and diagnostics. Called after bulk index operations so that previously-opened editors
/// immediately pick up the newly indexed symbol information.
pub(crate) async fn send_refresh_requests(client: &Client) {
    client.send_request::<SemanticTokensRefresh>(()).await.ok();
    client.send_request::<CodeLensRefresh>(()).await.ok();
    client
        .send_request::<InlayHintRefreshRequest>(())
        .await
        .ok();
    client
        .send_request::<WorkspaceDiagnosticRefresh>(())
        .await
        .ok();
    client
        .send_request::<InlineValueRefreshRequest>(())
        .await
        .ok();
}

/// Recursively scan `root` for `*.php` files and add them to the document store.
/// Skips hidden directories (names starting with `.`) and any path whose string
/// representation contains a segment matching one of the `exclude_paths` patterns,
/// **unless** that same path also matches an `include_paths` pattern (in which case
/// it is indexed).  Returns the number of files indexed.
///
/// Phase 1 — directory traversal: async, parallel via tokio JoinSet (one task per
///   directory, bounded by a 32-slot semaphore). Collects mtime+size alongside paths
///   so Phase 2b can build a cheap stat-based cache key without hashing file content.
/// Phase 2a — file reading: async, up to 64 concurrent reads (I/O-bound).
/// Phase 2b — parsing + indexing: parallel via rayon (CPU-bound, work-stealing pool).
///
/// Post-salsa: we only populate the DocumentStore here. The codebase is built
/// on demand by the salsa `codebase` query the first time a feature asks for
/// it — every indexed file's FileIndex, memoized thereafter.
#[tracing::instrument(
    skip(docs, open_files, cache, exclude_paths, include_paths),
    fields(root = %root.display())
)]
pub(crate) async fn scan_workspace(
    root: std::path::PathBuf,
    docs: Arc<DocumentStore>,
    open_files: OpenFiles,
    cache: Option<crate::cache::WorkspaceCache>,
    exclude_paths: &[String],
    include_paths: &[String],
    max_files: usize,
) -> usize {
    // Phase 1: synchronous directory walk in the blocking pool.
    //
    // The async version called next_entry().await and file_type().await for
    // every entry. On APFS/ext4 file_type() is a free readdir-buffer read, but
    // each .await still pays a full tokio yield+schedule cycle (~50 µs).
    // Across ~5 000 total entries that adds up to 100–200 ms of pure scheduler
    // overhead with zero I/O benefit. Moving the walk to spawn_blocking cuts
    // it to a handful of syscalls and eliminates the scheduler tax entirely.
    let root2 = root.clone();
    let excl: Vec<String> = exclude_paths.to_vec();
    let incl: Vec<String> = include_paths.to_vec();
    let php_paths: Vec<std::path::PathBuf> = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut stack = vec![root2.clone()];
        'walk: while let Some(dir) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for entry in rd.flatten() {
                let path = entry.path();
                let rel_path = path
                    .strip_prefix(&root2)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

                let is_excluded = matches_any(&rel_path, &excl);
                let is_included =
                    matches_include_prefix(&rel_path, &incl) || matches_any(&rel_path, &incl);
                if is_excluded && !is_included && !has_included_children(&rel_path, &incl) {
                    continue;
                }

                let ft = match entry.file_type() {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                if ft.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        stack.push(path);
                    }
                } else if ft.is_file() && path.extension().is_some_and(|e| e == "php") {
                    out.push(path);
                    if out.len() >= max_files {
                        break 'walk;
                    }
                }
            }
        }
        out
    })
    .await
    .unwrap_or_default();

    // Phase 2a: read files concurrently (I/O-bound).
    // mtime+size are fetched in Phase 2b via synchronous std::fs::metadata()
    // inside the rayon closure — that way the stat syscall (~5 µs) runs in the
    // blocking pool with no async scheduling overhead instead of adding an
    // extra async await per file here.
    let io_sem = Arc::new(tokio::sync::Semaphore::new(64));
    let mut read_set: tokio::task::JoinSet<Option<(Url, String)>> = tokio::task::JoinSet::new();

    for path in php_paths {
        let permit = Arc::clone(&io_sem).acquire_owned().await.unwrap();
        read_set.spawn(async move {
            let _permit = permit;
            let text = tokio::fs::read_to_string(&path).await.ok()?;
            let uri = Url::from_file_path(&path).ok()?;
            Some((uri, text))
        });
    }

    let mut file_contents: Vec<(Url, String)> = Vec::new();
    while let Some(Ok(Some(pair))) = read_set.join_next().await {
        file_contents.push(pair);
    }

    // Phase 2b: parse and index files in parallel (CPU-bound).
    // The cache key is derived from mtime+size via a synchronous std::fs::metadata()
    // call inside the rayon closure. In a blocking context this costs only the
    // stat syscall (~5 µs/file with no async scheduling overhead), far cheaper
    // than hashing the full file content (~1 ms/file CPU on warm starts).
    tokio::task::spawn_blocking(move || {
        let index_file = |(uri, text): &(Url, String)| -> usize {
            if open_files.contains(uri) {
                return 0;
            }

            let cache_key = cache.as_ref().and_then(|_| {
                let path = uri.to_file_path().ok()?;
                let meta = std::fs::metadata(&path).ok()?;
                let mtime_secs = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                Some(crate::cache::WorkspaceCache::key_for_stat(
                    uri.as_str(),
                    mtime_secs,
                    meta.len(),
                ))
            });
            if let (Some(cache), Some(key)) = (cache.as_ref(), cache_key.as_ref())
                && let Some(index) = cache.read::<crate::file_index::FileIndex>(key)
            {
                docs.mirror_text(uri, text);
                docs.seed_cached_index(uri, Arc::new(index));
                return 1;
            }

            let doc = parse_document_no_diags(text);
            if let (Some(cache), Some(key)) = (cache.as_ref(), cache_key.as_ref()) {
                let index = crate::file_index::FileIndex::extract(&doc);
                let _ = cache.write(key, &index);
                docs.mirror_text(uri, text);
                docs.seed_cached_index(uri, Arc::new(index));
            } else {
                docs.index_from_doc(uri.clone(), &doc);
            }
            1
        };

        let mut total = 0usize;
        for chunk in file_contents.chunks(500) {
            total += chunk.par_iter().map(index_file).sum::<usize>();
            docs.sync_workspace_files();
        }
        total
    })
    .await
    .unwrap_or(0)
}

fn matches_any(rel_path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| {
        let p = pat.trim_end_matches('*').trim_end_matches('/');
        rel_path.split('/').any(|c| c == p)
            || rel_path.starts_with(&format!("{p}/"))
            || rel_path.contains(&format!("/{p}/"))
            || rel_path
                .split('/')
                .any(|c| c.ends_with(".php") && c.strip_suffix(".php").unwrap_or(c) == p)
    })
}
fn matches_include_prefix(rel_path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| {
        let p = pat.trim_end_matches('*').trim_end_matches('/');
        rel_path.starts_with(&format!("{p}/")) || rel_path == p
    })
}
fn has_included_children(rel_path: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pat| {
        let p = pat.trim_end_matches('*').trim_end_matches('/');
        p.starts_with(&format!("{rel_path}/")) || p == rel_path
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    use rayon::prelude::*;
    use tower_lsp::lsp_types::Url;

    use super::scan_workspace;
    use crate::cache::WorkspaceCache;
    use crate::diagnostics::parse_document_no_diags;
    use crate::document_store::DocumentStore;
    use crate::open_files::OpenFiles;

    #[tokio::test]
    async fn cache_round_trip_writes_then_reads_file_index() {
        let src_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();

        std::fs::write(
            src_dir.path().join("Foo.php"),
            "<?php\nnamespace App;\nclass Foo { public function bar(): string {} }",
        )
        .unwrap();

        let cache = WorkspaceCache::with_dir(cache_dir.path().to_path_buf());

        // First scan: cache miss → parses file and writes cache entry.
        let docs1 = Arc::new(DocumentStore::new());
        let count1 = scan_workspace(
            src_dir.path().to_path_buf(),
            Arc::clone(&docs1),
            OpenFiles::default(),
            Some(cache.clone()),
            &[],
            &[],
            50_000,
        )
        .await;
        assert_eq!(count1, 1, "first scan should index 1 file");

        // Overwrite the cache entry with a sentinel. The scan now uses a
        // stat-based key (mtime + size), so we must derive the same key
        // from the file's actual metadata rather than from its content.
        let foo_path = src_dir.path().join("Foo.php");
        let uri = Url::from_file_path(&foo_path).unwrap();
        let meta = std::fs::metadata(&foo_path).unwrap();
        let mtime_secs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let sentinel = crate::file_index::FileIndex {
            namespace: Some("CACHE_HIT_MARKER".into()),
            ..Default::default()
        };
        let key = WorkspaceCache::key_for_stat(uri.as_str(), mtime_secs, meta.len());
        cache.write(&key, &sentinel).unwrap();

        // Second scan: same cache dir → must read the sentinel from disk.
        let docs2 = Arc::new(DocumentStore::new());
        let count2 = scan_workspace(
            src_dir.path().to_path_buf(),
            Arc::clone(&docs2),
            OpenFiles::default(),
            Some(cache.clone()),
            &[],
            &[],
            50_000,
        )
        .await;
        assert_eq!(count2, 1, "second scan should still index 1 file");

        let idx2 = docs2
            .snapshot_query_file_index(&uri)
            .expect("docs2 must have Foo.php indexed");

        assert_eq!(
            idx2.namespace.as_deref(),
            Some("CACHE_HIT_MARKER"),
            "second scan must use the on-disk cache, not re-parse"
        );
        assert!(
            idx2.classes.is_empty(),
            "sentinel has no classes; non-empty means cache was bypassed"
        );
    }

    #[tokio::test]
    async fn edit_clears_cached_index() {
        let src_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let php_path = src_dir.path().join("Bar.php");

        std::fs::write(
            &php_path,
            "<?php\nclass Bar { public function a(): void {} }",
        )
        .unwrap();

        let cache = WorkspaceCache::with_dir(cache_dir.path().to_path_buf());
        let docs = Arc::new(DocumentStore::new());

        // First scan: writes cache.
        scan_workspace(
            src_dir.path().to_path_buf(),
            Arc::clone(&docs),
            OpenFiles::default(),
            Some(cache.clone()),
            &[],
            &[],
            50_000,
        )
        .await;

        let uri = Url::from_file_path(&php_path).unwrap();
        let idx_before = docs
            .snapshot_query_file_index(&uri)
            .expect("Bar.php must be indexed");
        assert_eq!(idx_before.classes[0].methods.len(), 1);

        // Simulate an edit: mirror new text (clears cached_index).
        let new_src =
            "<?php\nclass Bar { public function a(): void {} public function b(): void {} }";
        docs.mirror_text(&uri, new_src);

        // Re-query: salsa should re-extract (2 methods now).
        let idx_after = docs
            .snapshot_query_file_index(&uri)
            .expect("Bar.php must still be indexed after edit");
        assert_eq!(
            idx_after.classes[0].methods.len(),
            2,
            "edit must invalidate cached_index so fresh parse + extract runs"
        );
    }

    /// Phase-by-phase profiling. Run in release for meaningful numbers:
    ///   cargo test -p php-lsp profile_scan_phases --release -- --ignored --nocapture
    #[ignore]
    #[tokio::test]
    async fn profile_scan_phases() {
        for root_str in ["/tmp/wordpress", "/tmp/laravel-framework"] {
            if !std::path::Path::new(root_str).is_dir() {
                println!("SKIP: {root_str} not found");
                continue;
            }
            profile_one(root_str).await;
        }
    }

    async fn profile_one(root_str: &str) {
        let root = std::path::PathBuf::from(root_str);
        let rayon_threads = rayon::current_num_threads();

        // ── Phase 1: async serial walk (current production code) ────────────
        let t0 = Instant::now();
        let mut php_paths: Vec<std::path::PathBuf> = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let mut rd = match tokio::fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };
            while let Ok(Some(entry)) = rd.next_entry().await {
                let path = entry.path();
                let ft = match entry.file_type().await {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                if ft.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        stack.push(path);
                    }
                } else if ft.is_file() && path.extension().is_some_and(|e| e == "php") {
                    php_paths.push(path);
                }
            }
        }
        let t_walk_async = t0.elapsed();

        // ── Phase 1 alternative: sync walk in spawn_blocking ────────────────
        let root2 = root.clone();
        let t1 = Instant::now();
        let _php_sync: Vec<std::path::PathBuf> = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            let mut stack = vec![root2];
            while let Some(dir) = stack.pop() {
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for entry in rd.flatten() {
                        let path = entry.path();
                        if let Ok(ft) = entry.file_type() {
                            if ft.is_dir() {
                                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                                if !name.starts_with('.') {
                                    stack.push(path);
                                }
                            } else if ft.is_file() && path.extension().is_some_and(|e| e == "php") {
                                out.push(path);
                            }
                        }
                    }
                }
            }
            out
        })
        .await
        .unwrap();
        let t_walk_sync = t1.elapsed();
        let n_files = php_paths.len();

        // ── Phase 2a: concurrent reads ──────────────────────────────────────
        let t2 = Instant::now();
        let sem = Arc::new(tokio::sync::Semaphore::new(64));
        let mut set: tokio::task::JoinSet<Option<(Url, String, usize)>> =
            tokio::task::JoinSet::new();
        for path in &php_paths {
            let path = path.clone();
            let permit = Arc::clone(&sem).acquire_owned().await.unwrap();
            set.spawn(async move {
                let _permit = permit;
                let text = tokio::fs::read_to_string(&path).await.ok()?;
                let bytes = text.len();
                let uri = Url::from_file_path(&path).ok()?;
                Some((uri, text, bytes))
            });
        }
        let mut file_contents: Vec<(Url, String)> = Vec::new();
        let mut total_bytes = 0usize;
        while let Some(Ok(Some((uri, text, bytes)))) = set.join_next().await {
            total_bytes += bytes;
            file_contents.push((uri, text));
        }
        let t_read = t2.elapsed();

        // ── Phase 2b-cold: parse + extract (rayon) ──────────────────────────
        let t3 = Instant::now();
        let parse_ns = Arc::new(AtomicU64::new(0));
        let extract_ns = Arc::new(AtomicU64::new(0));
        file_contents.par_iter().for_each(|(_, text)| {
            let tp = Instant::now();
            let doc = parse_document_no_diags(text);
            parse_ns.fetch_add(tp.elapsed().as_nanos() as u64, Ordering::Relaxed);
            let te = Instant::now();
            let _ = crate::file_index::FileIndex::extract(&doc);
            extract_ns.fetch_add(te.elapsed().as_nanos() as u64, Ordering::Relaxed);
        });
        let t_parse_wall = t3.elapsed();
        let parse_cpu_ms = parse_ns.load(Ordering::Relaxed) / 1_000_000;
        let extract_cpu_ms = extract_ns.load(Ordering::Relaxed) / 1_000_000;

        // ── Phase 2b-warm: mtime key + cache read (current production) ──────
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = WorkspaceCache::with_dir(cache_dir.path().to_path_buf());
        // Populate cache via stat key.
        file_contents.par_iter().for_each(|(uri, text)| {
            if let Some(path) = uri.to_file_path().ok() {
                if let Ok(meta) = std::fs::metadata(&path) {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let key = WorkspaceCache::key_for_stat(uri.as_str(), mtime, meta.len());
                    let doc = parse_document_no_diags(text);
                    let idx = crate::file_index::FileIndex::extract(&doc);
                    let _ = cache.write(&key, &idx);
                }
            }
        });
        let t4 = Instant::now();
        let stat_ns = Arc::new(AtomicU64::new(0));
        let cache_read_ns = Arc::new(AtomicU64::new(0));
        let hits = Arc::new(AtomicU64::new(0));
        file_contents.par_iter().for_each(|(uri, _)| {
            if let Some(path) = uri.to_file_path().ok() {
                let ts = Instant::now();
                let meta = std::fs::metadata(&path).ok();
                stat_ns.fetch_add(ts.elapsed().as_nanos() as u64, Ordering::Relaxed);
                if let Some(meta) = meta {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let key = WorkspaceCache::key_for_stat(uri.as_str(), mtime, meta.len());
                    let tr = Instant::now();
                    if cache.read::<crate::file_index::FileIndex>(&key).is_some() {
                        hits.fetch_add(1, Ordering::Relaxed);
                    }
                    cache_read_ns.fetch_add(tr.elapsed().as_nanos() as u64, Ordering::Relaxed);
                }
            }
        });
        let t_warm_wall = t4.elapsed();

        // ── Salsa sync ───────────────────────────────────────────────────────
        let docs = Arc::new(DocumentStore::new());
        for (uri, text) in &file_contents {
            docs.mirror_text(uri, text);
        }
        let t5 = Instant::now();
        docs.sync_workspace_files();
        let t_salsa = t5.elapsed();

        // ── Report ───────────────────────────────────────────────────────────
        let h = hits.load(Ordering::Relaxed) as usize;
        let stat_ms = stat_ns.load(Ordering::Relaxed) / 1_000_000;
        let cread_ms = cache_read_ns.load(Ordering::Relaxed) / 1_000_000;

        println!();
        println!("═══ {root_str} ═══");
        println!(
            "  {n_files} files  {:.1} MB  {rayon_threads} rayon threads",
            total_bytes as f64 / 1_048_576.0
        );
        println!();
        println!("Phase 1  async walk (current)  : {t_walk_async:.2?}");
        println!("Phase 1  sync  walk (blocking)  : {t_walk_sync:.2?}  ← potential gain");
        println!("Phase 2a reads  (64-concurrent) : {t_read:.2?}");
        println!();
        println!("Phase 2b COLD");
        println!("  wall (rayon {rayon_threads}T)         : {t_parse_wall:.2?}");
        println!(
            "  CPU parse                   : {parse_cpu_ms} ms  ({:.2} ms/file)",
            parse_cpu_ms as f64 / n_files as f64
        );
        println!(
            "  CPU extract                 : {extract_cpu_ms} ms  ({:.2} ms/file)",
            extract_cpu_ms as f64 / n_files as f64
        );
        println!(
            "  parallelism gain            : {:.1}×",
            (parse_cpu_ms + extract_cpu_ms) as f64 / t_parse_wall.as_millis() as f64
        );
        println!();
        println!("Phase 2b WARM (mtime key)");
        println!(
            "  wall (rayon {rayon_threads}T)         : {t_warm_wall:.2?}  ({h}/{n_files} hits)"
        );
        println!(
            "  CPU stat total              : {stat_ms} ms  ({:.3} ms/file)",
            stat_ms as f64 / n_files as f64
        );
        println!(
            "  CPU cache read total        : {cread_ms} ms  ({:.3} ms/file)",
            cread_ms as f64 / n_files as f64
        );
        println!();
        println!("Salsa sync ({n_files} files)          : {t_salsa:.2?}");
        println!();
        println!(
            "Cold bottleneck  parse {:.0}% + reads {:.0}% + walk {:.0}%",
            t_parse_wall.as_millis() as f64
                / (t_walk_async + t_read + t_parse_wall).as_millis() as f64
                * 100.0,
            t_read.as_millis() as f64 / (t_walk_async + t_read + t_parse_wall).as_millis() as f64
                * 100.0,
            t_walk_async.as_millis() as f64
                / (t_walk_async + t_read + t_parse_wall).as_millis() as f64
                * 100.0,
        );
    }
}
