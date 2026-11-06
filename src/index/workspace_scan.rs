use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use tower_lsp_server::Client;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::request::{
    CodeLensRefresh, InlayHintRefreshRequest, InlineValueRefreshRequest, SemanticTokensRefresh,
    WorkspaceDiagnosticRefresh,
};

use crate::document::document_store::DocumentStore;
use crate::document::open_files::OpenFiles;
use crate::index::{has_included_children, matches_any, matches_include_prefix, normalize_path};

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
/// Phase 1 — directory traversal: parallel via rayon in the blocking pool.
/// Phase 2a — file reading: async, up to 64 concurrent reads (I/O-bound).
/// Phase 2b — mirroring + indexing: `FileIndex` projected from mir's definitions,
///   parallel via rayon per chunk.
///
/// Progress update emitted after each indexing chunk: `(files_indexed, total_files)`.
pub(crate) type ScanProgressTx = tokio::sync::mpsc::UnboundedSender<(usize, usize)>;

/// Post-salsa: we only populate the DocumentStore here. The codebase is built
/// on demand by the salsa `codebase` query the first time a feature asks for
/// it — every indexed file's FileIndex, memoized thereafter.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    skip(docs, open_files, workspace_roots, exclude_paths, include_paths),
    fields(root = %root.display())
)]
/// Number of files added to the index.
pub(crate) async fn scan_workspace(
    root: std::path::PathBuf,
    docs: Arc<DocumentStore>,
    open_files: OpenFiles,
    workspace_roots: &[std::path::PathBuf],
    exclude_paths: &[String],
    include_paths: &[String],
    extensions: &mir_analyzer::PhpFileExtensions,
    max_files: usize,
    progress: Option<ScanProgressTx>,
) -> usize {
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
                            let abs = normalize_path(&proj_root.join(rel));
                            if extensions.is_php_source(&abs)
                                && abs.exists()
                                && !paths.contains(&abs)
                            {
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
    let exts = extensions.clone();
    let roots = workspace_roots.to_vec();
    let php_paths: Vec<std::path::PathBuf> = tokio::task::spawn_blocking(move || {
        let out = Mutex::new(Vec::new());
        let count = AtomicUsize::new(0);
        let links = SymlinkGuard::new(&root2, &roots);
        let ignores = GitIgnores::above(&root2);
        walk_dir_parallel(
            root2.clone(),
            &root2,
            &excl,
            &incl,
            &exts,
            max_files,
            &out,
            &count,
            &links,
            &ignores,
        );
        out.into_inner().unwrap()
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("workspace scan (phase 1 walk) panicked: {e}");
        Vec::new()
    });

    // Prepend explicit autoload.files so they are always indexed regardless of
    // whether the directory walk would reach them.
    let walked: Vec<std::path::PathBuf> = php_paths
        .into_iter()
        .filter(|p| !autoload_paths.contains(p))
        .collect();
    let php_paths: Vec<std::path::PathBuf> = autoload_paths.into_iter().chain(walked).collect();

    // Phase 2a: read files concurrently (I/O-bound).
    let io_sem = Arc::new(tokio::sync::Semaphore::new(64));
    let mut read_set: tokio::task::JoinSet<Option<(Uri, String)>> = tokio::task::JoinSet::new();

    for path in php_paths {
        let permit = Arc::clone(&io_sem).acquire_owned().await.unwrap();
        read_set.spawn(async move {
            let _permit = permit;
            let text = crate::text::decode_source(tokio::fs::read(&path).await.ok()?);
            let uri = Uri::from_file_path(&path)?;
            Some((uri, text))
        });
    }

    // Drain every spawned read. A `while let Some(Ok(Some(_)))` here would stop
    // the loop the moment one task resolves to `Ok(None)` (a file that failed to
    // read, e.g. permission denied) or `Err` (a join error), silently dropping every
    // not-yet-collected file — so a single unreadable file (common under
    // `vendor/`) could truncate the whole index. Match each result instead and
    // skip only the failures.
    let mut file_contents: Vec<(Uri, String)> = Vec::new();
    while let Some(res) = read_set.join_next().await {
        if let Ok(Some(pair)) = res {
            file_contents.push(pair);
        }
    }

    // Phase 2b: mirror text into salsa, then project each chunk's `FileIndex`
    // from mir's definitions in parallel (CPU-bound; mir's own disk cache
    // makes a returning file a deserialization rather than a parse).
    let total_files = file_contents.len();
    tokio::task::spawn_blocking(move || {
        let mut total = 0usize;
        for chunk in file_contents.chunks(500) {
            let mut mirrored: Vec<Uri> = Vec::with_capacity(chunk.len());
            for (uri, text) in chunk {
                // Requests the user is waiting on take priority over indexing:
                // pause before this file's salsa writes while any interactive
                // read is in flight, so its snapshot isn't repeatedly cancelled.
                docs.yield_to_interactive_reads();
                if open_files.contains(uri) {
                    continue;
                }
                docs.mirror_text(uri, text);
                mirrored.push(uri.clone());
            }
            docs.sync_workspace_files();
            docs.prewarm_file_indexes(&mirrored);
            total += mirrored.len();
            if let Some(ref tx) = progress {
                let _ = tx.send((total, total_files));
            }
        }
        total
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("workspace scan (phase 2b index) panicked: {e}");
        0
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
#[allow(clippy::too_many_arguments)]
fn walk_dir_parallel(
    dir: std::path::PathBuf,
    root: &std::path::Path,
    excl: &[String],
    incl: &[String],
    extensions: &mir_analyzer::PhpFileExtensions,
    max_files: usize,
    out: &Mutex<Vec<std::path::PathBuf>>,
    count: &AtomicUsize,
    links: &SymlinkGuard,
    ignores: &GitIgnores,
) {
    if count.load(AtomicOrdering::Relaxed) >= max_files {
        return;
    }
    let rd = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    let ignores = ignores.entering(&dir);

    let mut subdirs = Vec::new();
    let mut files = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let rel_path = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

        let ft = match entry.file_type() {
            Ok(f) => f,
            Err(_) => continue,
        };
        let (is_dir, is_file, link_target) = if ft.is_symlink() {
            match links.resolve(&path, &dir) {
                Some((target, meta)) => (meta.is_dir(), meta.is_file(), Some(target)),
                None => continue,
            }
        } else {
            (ft.is_dir(), ft.is_file(), None)
        };

        let is_excluded = matches_any(&rel_path, excl);
        let is_included = matches_include_prefix(&rel_path, incl) || matches_any(&rel_path, incl);
        let force_included = is_included || (is_dir && has_included_children(&rel_path, incl));
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_vendor_dir = is_dir && name == "vendor";
        if !force_included && (is_excluded || (!is_vendor_dir && ignores.is_ignored(&path, is_dir)))
        {
            continue;
        }
        if is_dir {
            if !name.starts_with('.') && link_target.is_none_or(|t| links.first_visit(t)) {
                subdirs.push(path);
            }
        } else if is_file && extensions.is_php_source(&path) {
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
        // `vendor/` is usually gitignored but is indexed deliberately.
        let unfiltered = GitIgnores::default();
        let sub_ignores = if sub.file_name().is_some_and(|n| n == "vendor") {
            &unfiltered
        } else {
            &ignores
        };
        walk_dir_parallel(
            sub,
            root,
            excl,
            incl,
            extensions,
            max_files,
            out,
            count,
            links,
            sub_ignores,
        );
    });
}

/// `.gitignore` rules for a directory, outermost first; inactive outside a repository or in `vendor/`.
#[derive(Clone, Default)]
struct GitIgnores {
    active: bool,
    rules: Vec<Arc<ignore::gitignore::Gitignore>>,
}

impl GitIgnores {
    /// Rules from the enclosing repository's root down to `root`'s parent.
    fn above(root: &std::path::Path) -> Self {
        let Some(repo_root) = root.ancestors().find(|dir| dir.join(".git").exists()) else {
            return Self::default();
        };
        let mut ignores = Self {
            active: true,
            rules: Vec::new(),
        };
        let parents: Vec<_> = root
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(repo_root))
            .collect();
        for dir in parents.into_iter().rev() {
            ignores = ignores.entering(dir);
        }
        ignores
    }

    fn entering(&self, dir: &std::path::Path) -> Self {
        let mut next = self.clone();
        let file = dir.join(".gitignore");
        if self.active && file.is_file() {
            let (rules, _partial_errors) = ignore::gitignore::Gitignore::new(file);
            next.rules.push(Arc::new(rules));
        }
        next
    }

    fn is_ignored(&self, path: &std::path::Path, is_dir: bool) -> bool {
        self.rules
            .iter()
            .rev()
            .map(|rules| rules.matched(path, is_dir))
            .find(|m| !m.is_none())
            .is_some_and(|m| m.is_ignore())
    }
}

/// Decides which symlinks the scan follows. Only links whose target lies
/// outside every workspace root are followed (targets inside one are reached
/// directly, and following them would index the same file twice), and a
/// directory link is entered once and never when it points at an ancestor.
pub(crate) struct SymlinkGuard {
    canonical_root: std::path::PathBuf,
    other_roots: Vec<std::path::PathBuf>,
    visited: Mutex<std::collections::HashSet<std::path::PathBuf>>,
}

impl SymlinkGuard {
    pub(crate) fn new(root: &std::path::Path, workspace_roots: &[std::path::PathBuf]) -> Self {
        let canonical =
            |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        SymlinkGuard {
            canonical_root: canonical(root),
            other_roots: workspace_roots.iter().map(|r| canonical(r)).collect(),
            visited: Mutex::default(),
        }
    }

    /// The canonical target and metadata of `link` (found in `parent`) when
    /// it should be followed.
    fn resolve(
        &self,
        link: &std::path::Path,
        parent: &std::path::Path,
    ) -> Option<(std::path::PathBuf, std::fs::Metadata)> {
        let target = std::fs::canonicalize(link).ok()?;
        if target.starts_with(&self.canonical_root)
            || self.canonical_root.starts_with(&target)
            || self.other_roots.iter().any(|r| target.starts_with(r))
        {
            return None;
        }
        let meta = std::fs::metadata(&target).ok()?;
        if meta.is_dir() && std::fs::canonicalize(parent).ok()?.starts_with(&target) {
            return None;
        }
        Some((target, meta))
    }

    fn first_visit(&self, target: std::path::PathBuf) -> bool {
        self.visited.lock().unwrap().insert(target)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tower_lsp_server::ls_types::Uri;

    use super::scan_workspace;
    use crate::document::document_store::DocumentStore;
    use crate::document::open_files::OpenFiles;

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
        let extensions: &'static mir_analyzer::PhpFileExtensions = Box::leak(Box::default());
        let scan = tokio::spawn(scan_workspace(
            src_dir.path().to_path_buf(),
            Arc::clone(&docs),
            OpenFiles::default(),
            &[],
            &[],
            &[],
            extensions,
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
        let indexed = scan.await.unwrap();
        assert_eq!(indexed, 3, "scan must resume and index everything");
    }

    fn write_php(root: &std::path::Path, rel: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "<?php\nfunction f() {}").unwrap();
    }

    async fn scan_count(root: &std::path::Path, include: &[String]) -> usize {
        scan_workspace(
            root.to_path_buf(),
            Arc::new(DocumentStore::new()),
            OpenFiles::default(),
            &[],
            &[],
            include,
            &Default::default(),
            50_000,
            None,
        )
        .await
    }

    #[tokio::test]
    async fn scan_skips_gitignored_files_but_keeps_vendor() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".gitignore"),
            "/vendor/\nfixtures/\n*.gen.php\n!keep.gen.php\n",
        )
        .unwrap();
        write_php(dir.path(), "src/App.php");
        write_php(dir.path(), "src/keep.gen.php");
        write_php(dir.path(), "src/drop.gen.php");
        write_php(dir.path(), "fixtures/a/b.php");
        write_php(dir.path(), "vendor/acme/fixtures/Lib.php");
        write_php(dir.path(), "src/nested/kept.php");
        write_php(dir.path(), "src/nested/local.php");
        std::fs::write(dir.path().join("src/nested/.gitignore"), "local.php\n").unwrap();

        assert_eq!(scan_count(dir.path(), &[]).await, 4);
    }

    #[tokio::test]
    async fn include_paths_override_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "generated/\n").unwrap();
        write_php(dir.path(), "generated/Model.php");

        assert_eq!(scan_count(dir.path(), &[]).await, 0);
        assert_eq!(scan_count(dir.path(), &["generated".to_string()]).await, 1);
    }

    #[tokio::test]
    async fn gitignore_in_repo_root_applies_to_a_subdirectory_workspace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "app/cache/\n").unwrap();
        write_php(dir.path(), "app/src/A.php");
        write_php(dir.path(), "app/cache/B.php");

        assert_eq!(scan_count(&dir.path().join("app"), &[]).await, 1);
    }

    #[tokio::test]
    async fn gitignore_outside_a_repository_is_not_applied() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "generated/\n").unwrap();
        write_php(dir.path(), "generated/Model.php");

        assert_eq!(scan_count(dir.path(), &[]).await, 1);
    }

    #[tokio::test]
    async fn scan_indexes_only_configured_extensions() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.php", "b.module", "c.inc", "d.txt"] {
            std::fs::write(dir.path().join(name), "<?php\nfunction f() {}").unwrap();
        }
        let scan = |exts: mir_analyzer::PhpFileExtensions| {
            let root = dir.path().to_path_buf();
            async move {
                scan_workspace(
                    root,
                    Arc::new(DocumentStore::new()),
                    OpenFiles::default(),
                    &[],
                    &[],
                    &[],
                    &exts,
                    50_000,
                    None,
                )
                .await
            }
        };
        assert_eq!(scan(Default::default()).await, 1);
        assert_eq!(
            scan(mir_analyzer::PhpFileExtensions::new([
                "php", ".module", "inc"
            ]))
            .await,
            3
        );
    }

    #[tokio::test]
    async fn edit_reindexes_file() {
        let src_dir = tempfile::tempdir().unwrap();
        let php_path = src_dir.path().join("Bar.php");

        std::fs::write(
            &php_path,
            "<?php\nclass Bar { public function a(): void {} }",
        )
        .unwrap();

        let docs = Arc::new(DocumentStore::new());

        scan_workspace(
            src_dir.path().to_path_buf(),
            Arc::clone(&docs),
            OpenFiles::default(),
            &[],
            &[],
            &[],
            &Default::default(),
            50_000,
            None,
        )
        .await;

        let uri = Uri::from_file_path(&php_path).unwrap();
        let idx_before = docs
            .snapshot_query_file_index(&uri)
            .expect("Bar.php must be indexed");
        assert_eq!(idx_before.classes[0].methods.len(), 1);
        assert_eq!(idx_before.classes[0].methods[0].name.as_ref(), "a");

        // Simulate an edit.
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
            "edit must re-index the file"
        );
        let method_names: Vec<&str> = idx_after.classes[0]
            .methods
            .iter()
            .map(|m| m.name.as_ref())
            .collect();
        assert_eq!(method_names, vec!["a", "b"]);
    }
}
