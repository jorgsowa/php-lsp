use std::sync::Arc;

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
/// Phase 2 — file reading + parsing: concurrent, bounded by available CPU cores.
///
/// Post-salsa: we only populate the DocumentStore here. The codebase is built
/// on demand by the salsa `codebase` query the first time a feature asks for
/// it — stubs + every indexed file's StubSlice, memoized thereafter.
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
                    rel_path.starts_with(&format!("{}/", p))
                        || rel_path == p
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
            let rel_path = path.strip_prefix(&root)
                .map(|p| p.to_string_lossy().replace('\\', "/").trim_start_matches('/').to_string())
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

    // Phase 2: read and parse files concurrently, bounded by available CPU cores.
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let sem = Arc::new(tokio::sync::Semaphore::new(parallelism));
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    for path in php_files {
        let permit = Arc::clone(&sem).acquire_owned().await.unwrap();
        let docs = Arc::clone(&docs);
        let open_files = open_files.clone();
        let cache = cache.clone();
        let count = Arc::clone(&count);
        set.spawn(async move {
            let _permit = permit;
            let Ok(text) = tokio::fs::read_to_string(&path).await else {
                return;
            };
            let Ok(uri) = Url::from_file_path(&path) else {
                return;
            };
            tokio::task::spawn_blocking(move || {
                // Skip files the editor has already opened — their buffer
                // is authoritative; scan must not overwrite their salsa
                // input with disk contents.
                if open_files.contains(&uri) {
                    return;
                }

                // Phase K2b read path: if the on-disk cache has a StubSlice
                // for this (uri, content) key, mirror the text and seed
                // the cached slice — `file_definitions` will return it
                // directly on the first query, skipping parse and
                // `DefinitionCollector` entirely. An edit later clears
                // the seeded slice via `mirror_text` (K2a).
                let cache_key = cache
                    .as_ref()
                    .map(|_| crate::cache::WorkspaceCache::key_for(uri.as_str(), &text));
                if let (Some(cache), Some(key)) = (cache.as_ref(), cache_key.as_ref())
                    && let Some(slice) = cache.read::<mir_codebase::storage::StubSlice>(key)
                {
                    docs.mirror_text(&uri, &text);
                    docs.seed_cached_slice(&uri, Arc::new(slice));
                    count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return;
                }

                // Cache miss: normal parse + mirror.
                let doc = parse_document_no_diags(&text);
                docs.index_from_doc(uri.clone(), &doc);
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // K2b write path: force `file_definitions` and persist
                // the fresh slice so a subsequent startup hits the cache.
                // The work is unavoidable anyway — `get_codebase_salsa`
                // would call `file_definitions` lazily on first use — so
                // materializing it here trades a small up-front cost for
                // a large warm-start win next time. Best-effort: a write
                // error is logged via `.ok()` and doesn't fail the scan.
                if let (Some(cache), Some(key)) = (cache.as_ref(), cache_key.as_ref())
                    && let Some(slice) = docs.slice_for(&uri)
                {
                    let _ = cache.write(key, &*slice);
                }
            })
            .await
            .ok();
        });
    }

    while set.join_next().await.is_some() {}

    count.load(std::sync::atomic::Ordering::Relaxed)
}
