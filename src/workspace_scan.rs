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
    // Phase 1: serial async directory walk (stack-based DFS).
    // file_type() reads d_type from the readdir buffer on APFS/ext4 — no extra
    // syscall for non-PHP entries. Stats for mtime+size are deferred to Phase 2a
    // where they run concurrently alongside the file reads.
    let mut php_paths: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![root.clone()];

    'walk: while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let rel_path = path
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

            let is_excluded = matches_any(&rel_path, exclude_paths);
            let is_included = matches_include_prefix(&rel_path, include_paths)
                || matches_any(&rel_path, include_paths);
            if is_excluded && !is_included && !has_included_children(&rel_path, include_paths) {
                continue;
            }

            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !name.starts_with('.') {
                    stack.push(path);
                }
            } else if file_type.is_file() && path.extension().is_some_and(|e| e == "php") {
                php_paths.push(path);
                if php_paths.len() >= max_files {
                    break 'walk;
                }
            }
        }
    }

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

    /// Detailed phase-by-phase profiling of the scan pipeline.
    ///
    /// Run with:
    ///   cargo test -p php-lsp profile_scan_phases -- --ignored --nocapture
    ///
    /// Requires /tmp/wordpress (see tests/workspace/feature_indexing_perf.rs).
    #[ignore]
    #[tokio::test]
    async fn profile_scan_phases() {
        const ROOT: &str = "/tmp/wordpress";
        if !std::path::Path::new(ROOT).is_dir() {
            println!("SKIP: {ROOT} not found");
            return;
        }

        let root = std::path::PathBuf::from(ROOT);
        let rayon_threads = rayon::current_num_threads();

        // ── Phase 1: directory walk ──────────────────────────────────────────
        let t0 = Instant::now();
        let mut php_files: Vec<std::path::PathBuf> = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(e) => e,
                Err(_) => continue,
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let file_type = match entry.file_type().await {
                    Ok(ft) => ft,
                    Err(_) => continue,
                };
                if file_type.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') {
                        stack.push(path);
                    }
                } else if file_type.is_file() && path.extension().is_some_and(|e| e == "php") {
                    php_files.push(path);
                }
            }
        }
        let t_walk = t0.elapsed();
        let n_files = php_files.len();

        // ── Phase 2a: concurrent file reads ────────────────────────────────
        let t1 = Instant::now();
        let sem = Arc::new(tokio::sync::Semaphore::new(64));
        let mut set: tokio::task::JoinSet<Option<(Url, String, usize)>> =
            tokio::task::JoinSet::new();
        for path in php_files {
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
        let t_read = t1.elapsed();

        // ── Phase 2b-cold: parse only (no cache) ───────────────────────────
        let t2 = Instant::now();
        let parse_ns = Arc::new(AtomicU64::new(0));
        let extract_ns = Arc::new(AtomicU64::new(0));
        let _: Vec<_> = file_contents
            .par_iter()
            .map(|(_, text)| {
                let tp = Instant::now();
                let doc = parse_document_no_diags(text);
                parse_ns.fetch_add(tp.elapsed().as_nanos() as u64, Ordering::Relaxed);
                let te = Instant::now();
                let _ = crate::file_index::FileIndex::extract(&doc);
                extract_ns.fetch_add(te.elapsed().as_nanos() as u64, Ordering::Relaxed);
            })
            .collect();
        let t_parse_wall = t2.elapsed();
        let parse_cpu_ms = parse_ns.load(Ordering::Relaxed) / 1_000_000;
        let extract_cpu_ms = extract_ns.load(Ordering::Relaxed) / 1_000_000;

        // ── Phase 2b-warm: cache read only ─────────────────────────────────
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = WorkspaceCache::with_dir(cache_dir.path().to_path_buf());
        // Populate the cache first (silent pass).
        let _: Vec<_> = file_contents
            .par_iter()
            .map(|(uri, text)| {
                let key = WorkspaceCache::key_for(uri.as_str(), text);
                let doc = parse_document_no_diags(text);
                let idx = crate::file_index::FileIndex::extract(&doc);
                let _ = cache.write(&key, &idx);
            })
            .collect();

        // Now measure cache read path.
        let t3 = Instant::now();
        let hash_ns = Arc::new(AtomicU64::new(0));
        let cache_read_ns = Arc::new(AtomicU64::new(0));
        let cache_hits = Arc::new(AtomicU64::new(0));
        let _: Vec<_> = file_contents
            .par_iter()
            .map(|(uri, text)| {
                let th = Instant::now();
                let key = WorkspaceCache::key_for(uri.as_str(), text);
                hash_ns.fetch_add(th.elapsed().as_nanos() as u64, Ordering::Relaxed);
                let tr = Instant::now();
                let hit = cache.read::<crate::file_index::FileIndex>(&key).is_some();
                cache_read_ns.fetch_add(tr.elapsed().as_nanos() as u64, Ordering::Relaxed);
                if hit {
                    cache_hits.fetch_add(1, Ordering::Relaxed);
                }
            })
            .collect();
        let t_cache_wall = t3.elapsed();

        // ── Phase 2b: salsa sync cost ───────────────────────────────────────
        let docs = Arc::new(DocumentStore::new());
        let open = crate::open_files::OpenFiles::default();
        // Populate docs with all files (mirrors text only, no parse).
        for (uri, text) in &file_contents {
            docs.mirror_text(uri, text);
        }
        let t4 = Instant::now();
        docs.sync_workspace_files();
        let t_salsa_sync = t4.elapsed();

        // ── Report ──────────────────────────────────────────────────────────
        let hits = cache_hits.load(Ordering::Relaxed) as usize;
        let hash_ms = hash_ns.load(Ordering::Relaxed) / 1_000_000;
        let cread_ms = cache_read_ns.load(Ordering::Relaxed) / 1_000_000;

        println!();
        println!("═══ Scan profile: {ROOT} ═══");
        println!("  rayon threads   : {rayon_threads}");
        println!("  files           : {n_files}");
        println!(
            "  total source    : {:.1} MB",
            total_bytes as f64 / 1_048_576.0
        );
        println!();
        println!("Phase 1  dir walk         : {t_walk:.2?}  (serial async)");
        println!("Phase 2a file reads       : {t_read:.2?}  (64 concurrent, {n_files} files)");
        println!();
        println!("Phase 2b COLD (parse path):");
        println!("  wall time (rayon)       : {t_parse_wall:.2?}");
        println!(
            "  CPU parse total         : {parse_cpu_ms} ms  ({:.1} ms/file avg)",
            parse_cpu_ms as f64 / n_files as f64
        );
        println!(
            "  CPU extract total       : {extract_cpu_ms} ms  ({:.1} ms/file avg)",
            extract_cpu_ms as f64 / n_files as f64
        );
        println!(
            "  parallelism gain        : {:.1}×  ({rayon_threads} threads)",
            (parse_cpu_ms + extract_cpu_ms) as f64 / t_parse_wall.as_millis() as f64
        );
        println!();
        println!("Phase 2b WARM (cache path):");
        println!("  wall time (rayon)       : {t_cache_wall:.2?}  ({hits}/{n_files} hits)");
        println!(
            "  CPU blake3 hash total   : {hash_ms} ms  ({:.2} ms/file avg)",
            hash_ms as f64 / n_files as f64
        );
        println!(
            "  CPU cache read total    : {cread_ms} ms  ({:.2} ms/file avg)",
            cread_ms as f64 / n_files as f64
        );
        println!();
        println!("Salsa sync (one call, {n_files} files): {t_salsa_sync:.2?}");
        println!();
        println!(
            "Bottleneck on cold start  : parse ({:.0}% of 2b wall)",
            parse_cpu_ms as f64 / rayon_threads as f64 / t_parse_wall.as_millis() as f64 * 100.0
        );
        println!("Bottleneck on warm start  : cache read + blake3 hash");
        println!("Mir involvement in scan   : NONE (mir runs on demand for diagnostics only)");
    }
}
