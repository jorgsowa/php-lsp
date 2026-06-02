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
/// Phase 1 — directory traversal: async, serial (I/O-bound; tokio handles it well).
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
    // Phase 1: collect PHP file paths via async directory walk.
    let mut php_files: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![root.clone()];

    'walk: while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();

            /// Check whether `rel_path` matches any of the given pattern list,
            /// using component-based matching (same semantics as the existing
            /// exclude logic).  Returns `true` if at least one pattern matches.
            fn matches_any(rel_path: &str, patterns: &[String]) -> bool {
                patterns.iter().any(|pat| {
                    let p = pat.trim_end_matches('*').trim_end_matches('/');
                    rel_path.split('/').any(|component| component == p)
                        || rel_path.starts_with(&format!("{}/", p))
                        || rel_path.contains(&format!("/{}/", p))
                        // Also match by file stem (filename without .php extension).
                        // This allows patterns like "Greeter" to match "src/Service/Greeter.php".
                        || rel_path.split('/').any(|component| {
                            component.ends_with(".php")
                                && component.strip_suffix(".php").unwrap_or(component) == p
                        })
                })
            }

            /// Check whether `rel_path` matches any of the given patterns as a prefix,
            /// i.e. the path starts with one of the pattern components followed by `/`.
            fn matches_include_prefix(rel_path: &str, patterns: &[String]) -> bool {
                patterns.iter().any(|pat| {
                    let p = pat.trim_end_matches('*').trim_end_matches('/');
                    rel_path.starts_with(&format!("{}/", p)) || rel_path == p
                })
            }

            /// Check whether `rel_path` has any included children — used to decide
            /// whether a directory that matches an exclude pattern should still be
            /// walked (because it contains sub-paths matching include patterns).
            fn has_included_children(rel_path: &str, patterns: &[String]) -> bool {
                patterns.iter().any(|pat| {
                    let p = pat.trim_end_matches('*').trim_end_matches('/');
                    // Check if any include pattern is a descendant of rel_path.
                    // Example: rel_path="vendor", p="vendor/yiisoft"
                    // → "vendor/yiisoft".starts_with("vendor/") == true ✓
                    p.starts_with(&format!("{}/", rel_path)) || p == rel_path
                })
            }

            // Compute a relative path from root so that patterns like
            // "vendor" and "vendor/yiisoft" match correctly.
            let rel_path = path
                .strip_prefix(&root)
                .map(|p| {
                    p.to_string_lossy()
                        .replace('\\', "/")
                        .trim_start_matches('/')
                        .to_string()
                })
                .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

            // Determine if this entry is excluded or included.
            let is_excluded = matches_any(&rel_path, exclude_paths);
            let is_included = matches_include_prefix(&rel_path, include_paths)
                || matches_any(&rel_path, include_paths);

            // Skip excluded paths unless they are explicitly included or contain
            // included children (e.g., "vendor/yiisoft" inside excluded "vendor/").
            if is_excluded && !is_included && !has_included_children(&rel_path, include_paths) {
                continue;
            }

            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };

            if file_type.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // Skip hidden directories; vendor is indexed unless excluded above.
                if !name.starts_with('.') {
                    stack.push(path);
                }
            } else if file_type.is_file() && path.extension().is_some_and(|e| e == "php") {
                php_files.push(path);
                if php_files.len() >= max_files {
                    break 'walk;
                }
            }
        }
    }

    // Phase 2a: read files concurrently (I/O-bound).
    // A semaphore of 64 avoids saturating the OS file-descriptor table while
    // still allowing substantial I/O parallelism independent of CPU count.
    let io_sem = Arc::new(tokio::sync::Semaphore::new(64));
    let mut read_set: tokio::task::JoinSet<Option<(Url, String)>> = tokio::task::JoinSet::new();

    for path in php_files {
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
    // Files are processed in chunks of 500; after each chunk the workspace
    // salsa input is synced so cross-file features (hover, definition,
    // workspace symbols) become available incrementally rather than waiting
    // for the full scan to complete.
    tokio::task::spawn_blocking(move || {
        let index_file = |(uri, text): &(Url, String)| -> usize {
            if open_files.contains(uri) {
                return 0;
            }

            let cache_key = cache
                .as_ref()
                .map(|_| crate::cache::WorkspaceCache::key_for(uri.as_str(), text));
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
            // Sync after each chunk so workspace queries see these files.
            docs.sync_workspace_files();
        }
        total
    })
    .await
    .unwrap_or(0)
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

        // Overwrite the cache entry with a sentinel value. If the second scan
        // actually reads from the cache it must return this sentinel; if it
        // silently falls through to parse, it would return real data and the
        // assertion below would catch the bug.
        let disk_content = "<?php\nnamespace App;\nclass Foo { public function bar(): string {} }";
        let uri = Url::from_file_path(src_dir.path().join("Foo.php")).unwrap();
        let sentinel = crate::file_index::FileIndex {
            namespace: Some("CACHE_HIT_MARKER".into()),
            ..Default::default()
        };
        let key = WorkspaceCache::key_for(uri.as_str(), disk_content);
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
