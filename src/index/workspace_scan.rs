use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::request::{
    CodeLensRefresh, InlayHintRefreshRequest, InlineValueRefreshRequest, SemanticTokensRefresh,
    WorkspaceDiagnosticRefresh,
};

use crate::analysis::diagnostics::parse_document_no_diags;
use crate::document::document_store::DocumentStore;
use crate::document::open_files::OpenFiles;

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
/// Progress update emitted after each indexing chunk: `(files_indexed, total_files)`.
pub(crate) type ScanProgressTx = tokio::sync::mpsc::UnboundedSender<(usize, usize)>;

/// Post-salsa: we only populate the DocumentStore here. The codebase is built
/// on demand by the salsa `codebase` query the first time a feature asks for
/// it — every indexed file's FileIndex, memoized thereafter.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip(docs, open_files, cache, exclude_paths, include_paths),
    fields(root = %root.display())
)]
/// `(total_indexed, from_cache)` — files counted in the index and how many
/// came from the on-disk cache without re-parsing.
pub(crate) async fn scan_workspace(
    root: std::path::PathBuf,
    docs: Arc<DocumentStore>,
    open_files: OpenFiles,
    cache: Option<crate::index::cache::WorkspaceCache>,
    exclude_paths: &[String],
    include_paths: &[String],
    max_files: usize,
    progress: Option<ScanProgressTx>,
) -> (usize, usize) {
    // Phase 1: synchronous directory walk in the blocking pool.
    //
    // The async version called next_entry().await and file_type().await for
    // every entry. On APFS/ext4 file_type() is a free readdir-buffer read, but
    // each .await still pays a full tokio yield+schedule cycle (~50 µs).
    // Across ~5 000 total entries that adds up to 100–200 ms of pure scheduler
    // overhead with zero I/O benefit. Moving the walk to spawn_blocking cuts
    // it to a handful of syscalls and eliminates the scheduler tax entirely.
    // Collect explicit autoload.files entries from composer.json so helper
    // functions (tap, class_uses_recursive, …) are indexed and don't produce
    // false-positive UndefinedFunction diagnostics.
    // Walk up from `root` to find the nearest composer.json (handles the common
    // case where the LSP workspace root is a sub-directory like `src/`).
    let autoload_paths: Vec<std::path::PathBuf> = {
        let composer_dir: Option<std::path::PathBuf> = {
            let mut dir = root.as_path();
            let mut found = None;
            for _ in 0..4 {
                if dir.join("composer.json").exists() {
                    found = Some(dir.to_path_buf());
                    break;
                }
                match dir.parent() {
                    Some(p) => dir = p,
                    None => break,
                }
            }
            found
        };
        if let Some(proj_root) = composer_dir {
            let text = std::fs::read_to_string(proj_root.join("composer.json")).unwrap_or_default();
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                let mut paths = Vec::new();
                for key in ["autoload", "autoload-dev"] {
                    for file in json[key]["files"].as_array().unwrap_or(&vec![]) {
                        if let Some(rel) = file.as_str() {
                            let abs = proj_root.join(rel);
                            if abs.extension().is_some_and(|e| e == "php") && abs.exists() {
                                paths.push(abs);
                            }
                        }
                    }
                }
                paths
            } else {
                vec![]
            }
        } else {
            vec![]
        }
    };

    let autoload_uris: Vec<Uri> = autoload_paths
        .iter()
        .filter_map(Uri::from_file_path)
        .collect();
    if !autoload_uris.is_empty() {
        docs.set_autoload_uris(autoload_uris);
    }

    let root2 = root.clone();
    let excl: Vec<String> = exclude_paths.to_vec();
    let incl: Vec<String> = include_paths.to_vec();
    let php_paths: Vec<std::path::PathBuf> = tokio::task::spawn_blocking(move || {
        let out = Mutex::new(Vec::new());
        let count = AtomicUsize::new(0);
        walk_dir_parallel(root2.clone(), &root2, &excl, &incl, max_files, &out, &count);
        out.into_inner().unwrap()
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("workspace scan (phase 1 walk) panicked: {e}");
        Vec::new()
    });

    // Prepend explicit autoload.files so they are always indexed regardless of
    // whether the directory walk would reach them.
    let php_paths: Vec<std::path::PathBuf> = autoload_paths.into_iter().chain(php_paths).collect();

    // Phase 2a: read files concurrently (I/O-bound).
    // mtime+size are fetched in Phase 2b via synchronous std::fs::metadata()
    // inside the rayon closure — that way the stat syscall (~5 µs) runs in the
    // blocking pool with no async scheduling overhead instead of adding an
    // extra async await per file here.
    let io_sem = Arc::new(tokio::sync::Semaphore::new(64));
    let mut read_set: tokio::task::JoinSet<Option<(Uri, String)>> = tokio::task::JoinSet::new();

    for path in php_paths {
        let permit = Arc::clone(&io_sem).acquire_owned().await.unwrap();
        read_set.spawn(async move {
            let _permit = permit;
            let text = tokio::fs::read_to_string(&path).await.ok()?;
            let uri = Uri::from_file_path(&path)?;
            Some((uri, text))
        });
    }

    // Drain every spawned read. A `while let Some(Ok(Some(_)))` here would stop
    // the loop the moment one task resolves to `Ok(None)` (a file that failed to
    // read, e.g. non-UTF-8) or `Err` (a join error), silently dropping every
    // not-yet-collected file — so a single unreadable file (common under
    // `vendor/`) could truncate the whole index. Match each result instead and
    // skip only the failures.
    let mut file_contents: Vec<(Uri, String)> = Vec::new();
    while let Some(res) = read_set.join_next().await {
        if let Ok(Some(pair)) = res {
            file_contents.push(pair);
        }
    }

    // Phase 2b: parse and index files in parallel (CPU-bound).
    // File content is already read above (file_contents); computing the cache
    // key from content rather than mtime+size is zero extra I/O and avoids
    // stale hits when a size-preserving edit occurs within the same 1-second
    // mtime tick (common with formatters / single-char swaps).
    let total_files = file_contents.len();
    tokio::task::spawn_blocking(move || {
        let cache_hits = std::sync::atomic::AtomicUsize::new(0);

        let index_file = |(uri, text): &(Uri, String)| -> usize {
            // Requests the user is waiting on take priority over indexing:
            // pause before this file's salsa writes while any interactive
            // read is in flight, so its snapshot isn't repeatedly cancelled.
            docs.yield_to_interactive_reads();
            if open_files.contains(uri) {
                return 0;
            }

            let cache_key = cache
                .as_ref()
                .map(|_| crate::index::cache::WorkspaceCache::key_for(uri.as_str(), text));
            if let (Some(cache), Some(key)) = (cache.as_ref(), cache_key.as_ref())
                && let Some(index) = cache.read::<crate::index::file_index::FileIndex>(key)
            {
                docs.mirror_text(uri, text);
                docs.seed_cached_index(uri, Arc::new(index));
                cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return 1;
            }

            let doc = parse_document_no_diags(text);
            if let (Some(cache), Some(key)) = (cache.as_ref(), cache_key.as_ref()) {
                let index = crate::index::file_index::FileIndex::extract(&doc);
                let _ = cache.write(key, &index);
                docs.mirror_text(uri, text);
                docs.seed_cached_index(uri, Arc::new(index));
            } else {
                docs.ingest_from_doc(uri.clone(), &doc);
            }
            1
        };

        let mut total = 0usize;
        for chunk in file_contents.chunks(500) {
            total += chunk.par_iter().map(index_file).sum::<usize>();
            docs.sync_workspace_files();
            if let Some(ref tx) = progress {
                let _ = tx.send((total, total_files));
            }
        }
        let from_cache = cache_hits.load(std::sync::atomic::Ordering::Relaxed);
        (total, from_cache)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("workspace scan (phase 2b index) panicked: {e}");
        (0, 0)
    })
}

/// Recursively walk `dir`, collecting matching `.php` paths into `out`. One
/// directory's `read_dir` (a syscall-bound, inherently serial operation) runs
/// per call; the fan-out across its subdirectories runs on the rayon pool, so
/// a workspace with many directories (the common case — namespaces mirror
/// directory structure) parallelizes across cores instead of one thread
/// walking the whole tree. Real-world PHP corpora are typically 20-40 files
/// per directory but thousands of directories, so this is where the
/// parallelism actually pays off — a single huge flat directory would not
/// benefit, but also would not regress (falls back to one recursive call
/// doing all the work itself).
///
/// `max_files` is enforced exactly (matching the old serial walk's contract)
/// via a compare-exchange reservation on `count`: each directory's file batch
/// atomically claims only as much of the remaining budget as is left, so the
/// total pushed to `out` across every parallel branch never exceeds the cap.
fn walk_dir_parallel(
    dir: std::path::PathBuf,
    root: &std::path::Path,
    excl: &[String],
    incl: &[String],
    max_files: usize,
    out: &Mutex<Vec<std::path::PathBuf>>,
    count: &AtomicUsize,
) {
    if count.load(AtomicOrdering::Relaxed) >= max_files {
        return;
    }
    let rd = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return,
    };

    let mut subdirs = Vec::new();
    let mut files = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let rel_path = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

        let is_excluded = matches_any(&rel_path, excl);
        let is_included = matches_include_prefix(&rel_path, incl) || matches_any(&rel_path, incl);
        if is_excluded && !is_included && !has_included_children(&rel_path, incl) {
            continue;
        }

        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        if ft.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') {
                subdirs.push(path);
            }
        } else if ft.is_file() && path.extension().is_some_and(|e| e == "php") {
            files.push(path);
        }
    }

    if !files.is_empty() {
        let mut current = count.load(AtomicOrdering::Relaxed);
        let claimed = loop {
            if current >= max_files {
                break 0;
            }
            let take = files.len().min(max_files - current);
            match count.compare_exchange_weak(
                current,
                current + take,
                AtomicOrdering::Relaxed,
                AtomicOrdering::Relaxed,
            ) {
                Ok(_) => break take,
                Err(actual) => current = actual,
            }
        };
        if claimed > 0 {
            files.truncate(claimed);
            out.lock().unwrap().extend(files);
        }
    }

    subdirs.into_par_iter().for_each(|sub| {
        walk_dir_parallel(sub, root, excl, incl, max_files, out, count);
    });
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
    use tower_lsp_server::ls_types::Uri;

    use super::scan_workspace;
    use crate::analysis::diagnostics::parse_document_no_diags;
    use crate::document::document_store::DocumentStore;
    use crate::document::open_files::OpenFiles;
    use crate::index::cache::WorkspaceCache;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scan_pauses_while_an_interactive_read_is_in_flight() {
        let src_dir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            std::fs::write(
                src_dir.path().join(format!("F{i}.php")),
                format!("<?php\nclass F{i} {{}}"),
            )
            .unwrap();
        }

        let docs = Arc::new(DocumentStore::new());
        let guard = docs.interactive_read_guard();
        let scan = tokio::spawn(scan_workspace(
            src_dir.path().to_path_buf(),
            Arc::clone(&docs),
            OpenFiles::default(),
            None,
            &[],
            &[],
            50_000,
            None,
        ));

        // The per-file yield is bounded at 500 ms; a 3-file scan otherwise
        // finishes in a few ms, so still-running at 150 ms proves it paused.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(
            !scan.is_finished(),
            "scan should pause at file boundaries while a read guard is held"
        );

        drop(guard);
        let (indexed, _) = scan.await.unwrap();
        assert_eq!(indexed, 3, "scan must resume and index everything");
    }

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
            None,
        )
        .await;
        assert_eq!(count1.0, 1, "first scan should index 1 file");

        // Overwrite the cache entry with a sentinel. The scan uses a
        // content-based key, so derive the same key from the file content.
        let foo_path = src_dir.path().join("Foo.php");
        let uri = Uri::from_file_path(&foo_path).unwrap();
        let foo_content = std::fs::read_to_string(&foo_path).unwrap();
        let sentinel = crate::index::file_index::FileIndex {
            namespace: Some("CACHE_HIT_MARKER".into()),
            ..Default::default()
        };
        let key = WorkspaceCache::key_for(uri.as_str(), &foo_content);
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
            None,
        )
        .await;
        assert_eq!(count2.0, 1, "second scan should still index 1 file");

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
            None,
        )
        .await;

        let uri = Uri::from_file_path(&php_path).unwrap();
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
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for root in [
            manifest.join("benches/fixtures/laravel"),
            manifest.join("tests/fixtures/symfony-demo"),
        ] {
            if !root.is_dir() {
                println!("SKIP: {} not found", root.display());
                continue;
            }
            profile_one(root.to_str().unwrap()).await;
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

        // ── Phase 1 alternative: rayon-parallel walk (new production code) ──
        let root3 = root.clone();
        let t1b = Instant::now();
        let php_parallel: Vec<std::path::PathBuf> = tokio::task::spawn_blocking(move || {
            let out = super::Mutex::new(Vec::new());
            let count = super::AtomicUsize::new(0);
            super::walk_dir_parallel(root3.clone(), &root3, &[], &[], 50_000, &out, &count);
            out.into_inner().unwrap()
        })
        .await
        .unwrap();
        let t_walk_parallel = t1b.elapsed();
        assert_eq!(
            php_parallel.len(),
            n_files,
            "parallel walk must find the same file count as the serial walks"
        );

        // ── Phase 2a: concurrent reads ──────────────────────────────────────
        let t2 = Instant::now();
        let sem = Arc::new(tokio::sync::Semaphore::new(64));
        let mut set: tokio::task::JoinSet<Option<(Uri, String, usize)>> =
            tokio::task::JoinSet::new();
        for path in &php_paths {
            let path = path.clone();
            let permit = Arc::clone(&sem).acquire_owned().await.unwrap();
            set.spawn(async move {
                let _permit = permit;
                let text = tokio::fs::read_to_string(&path).await.ok()?;
                let bytes = text.len();
                let uri = Uri::from_file_path(&path)?;
                Some((uri, text, bytes))
            });
        }
        let mut file_contents: Vec<(Uri, String)> = Vec::new();
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
            let _ = crate::index::file_index::FileIndex::extract(&doc);
            extract_ns.fetch_add(te.elapsed().as_nanos() as u64, Ordering::Relaxed);
        });
        let t_parse_wall = t3.elapsed();
        let parse_cpu_ms = parse_ns.load(Ordering::Relaxed) / 1_000_000;
        let extract_cpu_ms = extract_ns.load(Ordering::Relaxed) / 1_000_000;

        // ── Phase 2b-warm: content key + cache read (current production) ────
        let cache_dir = tempfile::tempdir().unwrap();
        let cache = WorkspaceCache::with_dir(cache_dir.path().to_path_buf());
        // Populate cache via content key (text is already in memory).
        file_contents.par_iter().for_each(|(uri, text)| {
            let key = WorkspaceCache::key_for(uri.as_str(), text);
            let doc = parse_document_no_diags(text);
            let idx = crate::index::file_index::FileIndex::extract(&doc);
            let _ = cache.write(&key, &idx);
        });
        let t4 = Instant::now();
        let hash_ns = Arc::new(AtomicU64::new(0));
        let cache_read_ns = Arc::new(AtomicU64::new(0));
        let hits = Arc::new(AtomicU64::new(0));
        file_contents.par_iter().for_each(|(uri, text)| {
            let th = Instant::now();
            let key = WorkspaceCache::key_for(uri.as_str(), text);
            hash_ns.fetch_add(th.elapsed().as_nanos() as u64, Ordering::Relaxed);
            let tr = Instant::now();
            if cache
                .read::<crate::index::file_index::FileIndex>(&key)
                .is_some()
            {
                hits.fetch_add(1, Ordering::Relaxed);
            }
            cache_read_ns.fetch_add(tr.elapsed().as_nanos() as u64, Ordering::Relaxed);
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
        let hash_ms = hash_ns.load(Ordering::Relaxed) / 1_000_000;
        let cread_ms = cache_read_ns.load(Ordering::Relaxed) / 1_000_000;

        println!();
        println!("═══ {root_str} ═══");
        println!(
            "  {n_files} files  {:.1} MB  {rayon_threads} rayon threads",
            total_bytes as f64 / 1_048_576.0
        );
        println!();
        println!("Phase 1  async walk (old prod)  : {t_walk_async:.2?}");
        println!("Phase 1  sync  walk (serial)    : {t_walk_sync:.2?}");
        println!(
            "Phase 1  rayon walk (new prod)  : {t_walk_parallel:.2?}  ← {:.1}x vs sync serial",
            t_walk_sync.as_secs_f64() / t_walk_parallel.as_secs_f64().max(1e-9)
        );
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
        println!("Phase 2b WARM (content key)");
        println!(
            "  wall (rayon {rayon_threads}T)         : {t_warm_wall:.2?}  ({h}/{n_files} hits)"
        );
        println!(
            "  CPU blake3 hash total       : {hash_ms} ms  ({:.3} ms/file)",
            hash_ms as f64 / n_files as f64
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
