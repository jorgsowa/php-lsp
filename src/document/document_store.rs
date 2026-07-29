use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use arc_swap::{ArcSwap, ArcSwapOption};

use dashmap::{DashMap, DashSet};
use salsa::Setter;
use tower_lsp::lsp_types::{SemanticToken, Url};

use crate::db::mir_queries::{LspWorkspace, LspWsFile};
use crate::document::ast::ParsedDoc;
use crate::document::cache_registry::CacheRegistry;
use crate::index::file_index::FileIndex;
use crate::lang::autoload::Psr4Map;

pub struct DocumentStore {
    /// Per-file caches with unified eviction logic. See [`CacheRegistry`].
    caches: CacheRegistry,

    // ── Salsa-input storage ────────────────────────────────────────────────
    // Phase E4: `DocumentStore` is now a pure salsa-input wrapper. Open-file
    // state (live text, version token, parse-diagnostics cache) lives on
    // `Backend` in its `open_files` map; the set of files tracked by salsa
    // is exactly `source_files.keys()`.
    /// `Url -> LspWsFile` lookup on the shared mir db. Each `LspWsFile` pairs
    /// mir's `SourceFile` (the shared text input) with the optional warm-start
    /// `cached_index`. Created/updated through the `AnalysisSession`'s
    /// `with_db_mut` under the db write lock; reads run on cheap snapshot clones.
    lsp_ws_files: DashMap<Url, LspWsFile>,
    /// URIs that have been removed. Re-opening a deleted URI un-deletes it here
    /// and reuses the existing `LspWsFile` handle.
    deleted_uris: DashSet<Url>,
    /// Set to `true` when the set of tracked files changes (add or remove).
    /// `sync_workspace_files` skips the collect/sort/compare path when this
    /// is `false`, avoiding a lock acquisition on every LSP request.
    workspace_files_dirty: AtomicBool,
    /// Cached result of `workspace_file_paths()`. `None` means stale/never
    /// built — cleared by `mark_workspace_files_dirty` rather than shared
    /// with `workspace_files_dirty` itself, since `sync_workspace_files`
    /// destructively consumes that flag (`swap(false, ..)`) and a second
    /// consumer reading it would race for the same signal.
    workspace_file_paths_cache: ArcSwapOption<Vec<Arc<str>>>,
    /// `LspWorkspace` salsa input on the shared mir db: the project-file scoping
    /// set aggregated by `workspace_index`. Created lazily on first sync (the db
    /// is owned by the lazily-built `AnalysisSession`).
    lsp_workspace: Mutex<Option<LspWorkspace>>,
    /// Target PHP version (selects the `AnalysisSession`). Stored here since the
    /// converged db has no php-lsp `Workspace` input to carry it.
    php_version: Mutex<mir_analyzer::PhpVersion>,
    /// Shared PSR-4 namespace-to-path map. Shared with `Backend` via `Arc`
    /// so updates from `initialized` (when composer.json is loaded) are
    /// visible here without any additional wiring. `ArcSwap` makes reads
    /// lock-free — a poisoned guard can no longer crash a request handler.
    psr4: Arc<ArcSwap<Psr4Map>>,
    /// mir-analyzer's `AnalysisSession` — owns the workspace MirDb, runs
    /// Pass-2 analysis, and lazy-loads dependencies via PSR-4. Built lazily
    /// on first use; rebuilt when PHP version changes.
    analysis_session: Mutex<Option<(mir_analyzer::PhpVersion, Arc<mir_analyzer::AnalysisSession>)>>,
    /// Cache directory shared with the workspace file-index cache. When set,
    /// new `AnalysisSession`s are built with `with_cache_dir` so that stub
    /// parsing results survive server restarts.
    session_cache_dir: OnceLock<std::path::PathBuf>,
    /// URIs of autoload.files entries from composer.json. These define global
    /// helper functions (e.g. tap, class_uses_recursive in Laravel) that are
    /// not discoverable by namespace walk. Pre-ingested into the AnalysisSession
    /// before each file analysis so mir doesn't emit false UndefinedFunction.
    autoload_uris: std::sync::RwLock<Vec<Url>>,
    /// Set once the workspace scan's reference-index phase finishes, i.e. the
    /// `subtypes_of` map in `workspace_index` is complete. Visibility-derived
    /// scope narrowing that relies on the full subtype set (protected methods)
    /// only applies once this is `true`; before then it falls back to the full
    /// workspace scope so references are never under-reported.
    index_ready: AtomicBool,
    /// Monotonically increasing counter bumped on every actual text write
    /// (including file deletions). Long-running read operations (e.g.
    /// `session_references_to`) capture this value before starting and
    /// cancel themselves if it advances — avoiding stale results and
    /// unbounded retry loops after concurrent edits.
    write_revision: AtomicU64,
    /// Cancel token for the in-flight dependent-diagnostics sweep. A newer
    /// edit's sweep cancels the previous one via [`Self::begin_reanalyze`], so
    /// fast typing preempts stale workspace re-analysis rather than queueing
    /// behind it.
    reanalyze_cancel: Mutex<mir_analyzer::IndexCancel>,
    /// Cancel token for the in-flight analysis warm sweep
    /// ([`Self::warm_analysis_sweep`]); a newer sweep cancels the previous one
    /// via [`Self::begin_warm_sweep`].
    warm_sweep_cancel: Mutex<mir_analyzer::IndexCancel>,
    /// Warm sweeps that ran to completion (not cancelled). Observability only,
    /// surfaced via `$/php-lsp/debugStats` so benches/tests can await warmth.
    warm_sweeps_completed: AtomicU64,
    /// Count of in-flight interactive reads (requests the user is waiting on).
    /// The workspace scan yields at file boundaries while this is non-zero, so
    /// its per-file salsa writes can't starve a request's snapshot into an
    /// endless `Cancelled` retry loop. Advisory only — relaxed ordering.
    interactive_reads: AtomicU64,
}

/// RAII handle marking an interactive read in flight; see
/// [`DocumentStore::interactive_read_guard`].
pub struct InteractiveReadGuard<'a>(&'a AtomicU64);

impl Drop for InteractiveReadGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Analysis threads need big stacks: deep type-inference recursion on
/// pathological files overflows rayon's 2 MB default, and salsa 0.28's
/// `DependencyGraph::update_transferred_edges` recurses per transferred
/// dependent under contended parallel queries — observed blowing 16 MB on
/// the Laravel fixture (64 MB measured clean over repeated runs; the pages
/// are reserved, not committed). Size the global pool before any parallel
/// analysis runs; build_global fails harmlessly if a pool already exists.
/// Benches/tests driving `AnalysisSession` without a `DocumentStore` must
/// call this themselves — rayon otherwise lazily builds the pool with
/// default stacks on mir's first `par_iter`.
pub fn ensure_rayon_worker_stacks() {
    static RAYON_STACK: OnceLock<()> = OnceLock::new();
    RAYON_STACK.get_or_init(|| {
        let _ = rayon::ThreadPoolBuilder::new()
            .stack_size(64 * 1024 * 1024)
            .build_global();
    });
}

impl DocumentStore {
    pub fn new() -> Self {
        ensure_rayon_worker_stacks();
        DocumentStore {
            caches: CacheRegistry::new(),
            lsp_ws_files: DashMap::new(),
            deleted_uris: DashSet::new(),
            workspace_files_dirty: AtomicBool::new(true),
            workspace_file_paths_cache: ArcSwapOption::from(None),
            lsp_workspace: Mutex::new(None),
            php_version: Mutex::new(mir_analyzer::PhpVersion::LATEST),
            psr4: Arc::new(ArcSwap::from_pointee(Psr4Map::empty())),
            analysis_session: Mutex::new(None),
            session_cache_dir: OnceLock::new(),
            autoload_uris: std::sync::RwLock::new(Vec::new()),
            index_ready: AtomicBool::new(false),
            write_revision: AtomicU64::new(0),
            reanalyze_cancel: Mutex::new(mir_analyzer::IndexCancel::new()),
            warm_sweep_cancel: Mutex::new(mir_analyzer::IndexCancel::new()),
            warm_sweeps_completed: AtomicU64::new(0),
            interactive_reads: AtomicU64::new(0),
        }
    }

    /// Mark an interactive read (a request the user is waiting on) as in
    /// flight for the guard's lifetime. Background bulk writers poll
    /// [`Self::yield_to_interactive_reads`] between files and pause while any
    /// guard is live, giving the read a write-free window to complete.
    pub fn interactive_read_guard(&self) -> InteractiveReadGuard<'_> {
        self.interactive_reads.fetch_add(1, Ordering::Relaxed);
        InteractiveReadGuard(&self.interactive_reads)
    }

    /// Take an interactive-read guard, wait (bounded ~50 ms) for in-flight
    /// background writes to go quiet, and return the settled write revision.
    ///
    /// For user-facing sweeps that abort when [`Self::write_rev`] advances:
    /// snapshotting the revision this way ensures only genuine user edits —
    /// not the background scan the guard just paused — void the sweep.
    /// Sleeps; call from a blocking thread only.
    pub fn settled_write_rev_guard(&self) -> (InteractiveReadGuard<'_>, u64) {
        let guard = self.interactive_read_guard();
        // Post-scan there is no background write storm to settle; skip the
        // sleep so steady-state requests pay only the guard's atomic inc.
        if !self.is_index_ready() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
            let mut rev = self.write_rev();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(2));
                let now = self.write_rev();
                let quiet = now == rev;
                rev = now;
                if quiet || std::time::Instant::now() >= deadline {
                    break;
                }
            }
        }
        (guard, self.write_rev())
    }

    /// Pause the calling (background-writer) thread while interactive reads
    /// are in flight, up to 500 ms per call. Bounded so a wedged reader can
    /// only slow the scan, never stop it; callers invoke this per file, so
    /// sustained interactive traffic keeps yielding at each boundary.
    pub fn yield_to_interactive_reads(&self) {
        let mut waited = std::time::Duration::ZERO;
        const STEP: std::time::Duration = std::time::Duration::from_millis(2);
        const MAX: std::time::Duration = std::time::Duration::from_millis(500);
        while self.interactive_reads.load(Ordering::Relaxed) > 0 && waited < MAX {
            std::thread::sleep(STEP);
            waited += STEP;
        }
    }

    /// Install a fresh cancel token for a dependent-diagnostics sweep,
    /// cancelling whichever sweep was previously in flight. Pass the returned
    /// token to `reanalyze_dependents_cancellable`; when a newer edit calls
    /// this again the old token flips and that older sweep stops at its next
    /// file boundary instead of racing this one to publish stale diagnostics.
    pub fn begin_reanalyze(&self) -> mir_analyzer::IndexCancel {
        let fresh = mir_analyzer::IndexCancel::new();
        let mut guard = self.reanalyze_cancel.lock().unwrap();
        guard.cancel();
        *guard = fresh.clone();
        fresh
    }

    /// Install a fresh cancel token for an analysis warm sweep, cancelling any
    /// sweep previously in flight. Mirrors [`Self::begin_reanalyze`] but on a
    /// separate slot: edit-triggered diagnostic sweeps and background warming
    /// must not cancel each other.
    pub fn begin_warm_sweep(&self) -> mir_analyzer::IndexCancel {
        let fresh = mir_analyzer::IndexCancel::new();
        let mut guard = self.warm_sweep_cancel.lock().unwrap();
        guard.cancel();
        *guard = fresh.clone();
        fresh
    }

    /// Drive every workspace file through mir's memoized `analyze_file` query
    /// so later reference/rename requests are memo hits instead of cold
    /// analyses (~20 ms per file on real code — a few hundred candidate files
    /// makes an interactive request take seconds).
    ///
    /// `priority` files (and the files declaring the classes they reference —
    /// the user's working set) are warmed first, so requests against open
    /// files go warm within the sweep's first seconds even on workspaces
    /// where the full sweep takes much longer.
    ///
    /// Background-priority: processes files in small chunks, yielding to
    /// interactive reads at each boundary, and stops at the next boundary once
    /// `cancel` flips. Re-running after edits is cheap — unaffected files
    /// revalidate via salsa without re-analysis. Blocking; call from
    /// `spawn_blocking`.
    pub fn warm_analysis_sweep(&self, priority: &[Url], cancel: &mir_analyzer::IndexCancel) {
        // Dedicated thread with a generous stack: the serial prepare phase and
        // priority resolution recurse over real-world ASTs whose depth can
        // exceed the default 2 MiB thread stack (debug builds especially).
        // One pathological file must not abort the whole server process.
        std::thread::scope(|s| {
            let _ = std::thread::Builder::new()
                .name("php-lsp-warm-sweep".into())
                .stack_size(64 * 1024 * 1024)
                .spawn_scoped(s, || self.warm_analysis_sweep_inner(priority, cancel))
                .map(|h| h.join());
        });
    }

    fn warm_analysis_sweep_inner(&self, priority: &[Url], cancel: &mir_analyzer::IndexCancel) {
        const CHUNK: usize = 32;
        let front: Vec<Arc<str>> = self.sweep_priority_files(priority);
        let front_set: HashSet<&str> = front.iter().map(|f| f.as_ref()).collect();
        let files: Vec<Arc<str>> = front
            .iter()
            .cloned()
            .chain(
                self.lsp_ws_files
                    .iter()
                    .filter(|e| !self.deleted_uris.contains(e.key()))
                    .map(|e| Arc::<str>::from(e.key().as_str()))
                    .filter(|f| !front_set.contains(f.as_ref())),
            )
            .collect();
        drop(front_set);
        let session = self.analysis_session(self.workspace_php_version());
        let mut all_chunks_settled = true;
        for chunk in files.chunks(CHUNK) {
            if cancel.is_cancelled() {
                return;
            }
            self.yield_to_interactive_reads();
            // A concurrent write (e.g. another file being ingested) can land
            // mid-chunk and cancel the analysis snapshot. Retry the same
            // chunk immediately — `cancel` (not just the transient snapshot
            // cancellation) is the authoritative stop signal, so a retry loop
            // here only spins while an unrelated writer keeps landing, and
            // exits promptly once `cancel` itself flips (a real edit
            // superseding this sweep). Without the retry, a chunk silently
            // skipped here has no guaranteed follow-up: `warm_sweeps_completed`
            // must not count this sweep as covering files it never actually
            // analyzed, or a caller polling that counter (e.g. a test waiting
            // for the reference index to be fully warm) can observe "done"
            // while some files' postings were never written.
            loop {
                if cancel.is_cancelled() {
                    all_chunks_settled = false;
                    break;
                }
                if salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                    session.reanalyze_files_cancellable(chunk, cancel)
                }))
                .is_ok()
                {
                    break;
                }
            }
        }
        if !cancel.is_cancelled() && all_chunks_settled {
            // The sweep staged each analyzed file's reference postings into
            // mir's AnalysisCache; persist them so the next launch's
            // `warm_start_indexes` replays references index-warm. Flush
            // before publishing completion — observers of the counter may
            // rely on the postings being on disk.
            session.flush_analysis_cache();
            self.warm_sweeps_completed.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Persist mir's staged analysis-cache entries (reference postings) to
    /// disk. No-op when nothing changed since the last flush.
    pub fn flush_analysis_cache(&self) {
        self.analysis_session(self.workspace_php_version())
            .flush_analysis_cache();
    }

    /// Warm sweeps that ran to completion. See `$/php-lsp/debugStats`.
    pub fn warm_sweeps_completed(&self) -> u64 {
        self.warm_sweeps_completed.load(Ordering::Relaxed)
    }

    /// The sweep's front of the queue: `priority` files themselves plus the
    /// files declaring the classes they reference (type hints, `use` imports,
    /// `new`, `extends`, …) — the set a request against an open file most
    /// likely touches. Resolution goes through the memoized workspace index,
    /// matching by FQN with a short-name fallback.
    fn sweep_priority_files(&self, priority: &[Url]) -> Vec<Arc<str>> {
        if priority.is_empty() {
            return Vec::new();
        }
        let ws = self.get_workspace_index_salsa();
        let mut out: Vec<Arc<str>> = Vec::new();
        let mut seen: HashSet<Arc<str>> = HashSet::new();
        let push = |uri: &Url, out: &mut Vec<Arc<str>>, seen: &mut HashSet<Arc<str>>| {
            let f: Arc<str> = Arc::from(uri.as_str());
            if seen.insert(Arc::clone(&f)) {
                out.push(f);
            }
        };
        for uri in priority {
            push(uri, &mut out, &mut seen);
        }
        for uri in priority {
            let Some(doc) = self.get_doc_salsa(uri) else {
                continue;
            };
            for fqn in crate::navigation::references::collect_referenced_class_fqns(&doc) {
                let short = crate::text::fqn_short_name(&fqn);
                let Some(refs) = ws.classes_by_name.get(short) else {
                    continue;
                };
                for &r in refs {
                    if let Some((decl_uri, cls)) = ws.at(r)
                        && cls.fqn.trim_start_matches('\\') == fqn
                    {
                        push(decl_uri, &mut out, &mut seen);
                    }
                }
            }
        }
        out
    }

    /// Mark the workspace reference index as fully built. Called by the scan
    /// when its final phase completes (alongside `$/php-lsp/indexReady`).
    pub fn mark_index_ready(&self) {
        self.index_ready.store(true, Ordering::Release);
    }

    /// Whether the workspace reference index has finished building.
    pub fn is_index_ready(&self) -> bool {
        self.index_ready.load(Ordering::Acquire)
    }

    /// Snapshot of the write-revision counter. Long-running reads should
    /// capture this before starting and pass it to cancellable operations;
    /// if the counter advances, those operations abort and return empty rather
    /// than looping indefinitely against a newly-invalidated database.
    pub fn write_rev(&self) -> u64 {
        self.write_revision.load(Ordering::Acquire)
    }

    /// Set the directory used to persist stub-parse and analysis results across
    /// server restarts. Subsequent calls are silently ignored (`OnceLock`
    /// semantics).
    ///
    /// If a session was already built — an early request (e.g. the editor's
    /// restored-buffer `didOpen` racing `initialized`) builds one on demand —
    /// it was pinned in-memory-only: nothing it computes is ever flushed, and
    /// the next launch replays nothing, silently re-paying the whole cold
    /// cost every session. Drop it so the next `analysis_session()` rebuilds
    /// with the cache attached; the few analyses done that early re-ingest
    /// on demand.
    pub fn set_session_cache_dir(&self, dir: std::path::PathBuf) {
        if self.session_cache_dir.set(dir).is_err() {
            return;
        }
        let dropped = self.analysis_session.lock().unwrap().take().is_some();
        if dropped {
            self.drop_session_scoped_state();
        }
    }

    /// Register URIs discovered from composer.json `autoload.files` entries.
    /// These PHP files define global helper functions (e.g. `tap()` in Laravel)
    /// that are not class-resolvable via PSR-4. Clears `analysis_cache` so the
    /// next per-file analysis pre-ingests them into the AnalysisSession before
    /// running mir's FileAnalyzer.
    pub fn set_autoload_uris(&self, uris: Vec<Url>) {
        *self.autoload_uris.write().unwrap() = uris;
        self.caches.evict_analysis_all();
    }

    /// Get or build the `AnalysisSession` for the given PHP version. Rebuilds
    /// when the version changes (e.g. user flipped config). The session owns
    /// the shared salsa db and AnalysisCache; lazy-loads vendor files via the
    /// shared PSR-4 map. Built-in stubs are *not* pre-loaded: mir's
    /// `prepare_ast_for_analysis` ingests the stubs each analyzed file
    /// references, and [`crate::types::stub_members`] faults in single stub
    /// files for builtin member/hover lookups.
    pub fn analysis_session(
        &self,
        php_version: mir_analyzer::PhpVersion,
    ) -> Arc<mir_analyzer::AnalysisSession> {
        let mut guard = self.analysis_session.lock().unwrap();
        if let Some((cached_ver, session)) = guard.as_ref()
            && *cached_ver == php_version
        {
            return Arc::clone(session);
        }
        // Build a fresh session. Hand it the shared PSR-4 map so it can
        // lazy-resolve `UndefinedClass` candidates without us having to mirror
        // every vendor file upfront.
        let resolver: Arc<dyn mir_analyzer::ClassResolver> = self.psr4.load_full();
        // References/implementations are answered from mir's delta-maintained
        // inverted indexes (posting lists + subtype edges), which the session
        // keeps in step with every edit path.
        let mut builder =
            mir_analyzer::AnalysisSession::new(php_version).with_class_resolver(resolver);
        if let Some(dir) = self.session_cache_dir.get() {
            builder = builder.with_cache_dir(dir);
        }
        let session = Arc::new(builder);
        *guard = Some((php_version, Arc::clone(&session)));
        session
    }

    /// Current PHP version tracked by the workspace input.
    pub fn workspace_php_version(&self) -> mir_analyzer::PhpVersion {
        *self.php_version.lock().unwrap()
    }

    /// File URIs of all direct and transitive subclasses of `class_fqn`,
    /// resolved via mir's inheritance graph. Returns an empty vec when the mir
    /// session hasn't ingested the class yet (cold start, excluded paths).
    ///
    /// Used by `goto_implementation` and `subtypes` to scope their lookups to
    /// the correct files, fixing aliased `extends` and FQN-qualified forms that
    /// the raw-name `subtypes_of` map misses.
    pub fn class_subtype_urls(&self, class_fqn: &str) -> Vec<tower_lsp::lsp_types::Url> {
        let session = self.analysis_session(self.workspace_php_version());
        session
            .subtype_files(class_fqn)
            .into_iter()
            .filter_map(|p| tower_lsp::lsp_types::Url::parse(&p).ok())
            .collect()
    }

    /// Return the `Arc<ArcSwap<Psr4Map>>` so callers can share it.
    /// `Backend` clones this arc at construction time so writes
    /// (e.g. loading composer.json on `initialized`) are immediately visible
    /// to PSR-4 resolution during analysis without extra plumbing.
    pub fn psr4_arc(&self) -> Arc<ArcSwap<Psr4Map>> {
        Arc::clone(&self.psr4)
    }

    /// Durability for a file's salsa input: vendor files never change within a
    /// session, so salsa can skip re-validating their queries on user edits.
    fn input_durability(uri: &Url) -> salsa::Durability {
        if uri.as_str().contains("/vendor/") {
            salsa::Durability::HIGH
        } else {
            salsa::Durability::LOW
        }
    }

    /// The `LspWsFile` handle for `uri`, if it is mirrored and not deleted.
    fn lsp_ws_file(&self, uri: &Url) -> Option<LspWsFile> {
        if self.deleted_uris.contains(uri) {
            return None;
        }
        self.lsp_ws_files.get(uri).map(|e| *e)
    }

    /// Take a cheap snapshot of the shared mir db and run `f` on it, retrying
    /// on `salsa::Cancelled` (raised when a concurrent writer bumps the
    /// revision). Mirrors [`Self::snapshot_query`] for the converged db.
    ///
    /// The loop terminates as soon as the writer pauses long enough for one
    /// query to complete. Handlers that want an early exit under sustained
    /// write pressure should pass a write-rev closure (see `code_lenses`) and
    /// check it at coarser granularity rather than here.
    fn snapshot_mir_query<R>(&self, f: impl Fn(&mir_analyzer::db::MirDbStorage) -> R) -> R {
        use std::panic::AssertUnwindSafe;
        let _interactive = self.interactive_read_guard();
        let session = self.analysis_session(self.workspace_php_version());
        // Each iteration's snapshot clone MUST drop before the next snapshot:
        // a concurrent writer's salsa `set` holds the mir write lock and waits
        // for outstanding db handles to drop, while the next `snapshot_db` needs
        // the read lock — keeping the clone alive across the retry deadlocks.
        loop {
            let db = session.snapshot_db();
            match salsa::Cancelled::catch(AssertUnwindSafe(|| f(&db))) {
                Ok(r) => return r,
                Err(_) => drop(db),
            }
        }
    }

    /// Bounded variant of [`Self::snapshot_mir_query`] for cosmetic,
    /// cursor-triggered reads that must not spin under sustained write pressure.
    /// Returns `None` if a concurrent writer cancels the query `attempts` times
    /// in a row (the caller then falls back to a stale result). Same
    /// drop-before-retry discipline as `snapshot_mir_query`.
    fn try_snapshot_mir_query<R>(
        &self,
        attempts: usize,
        f: impl Fn(&mir_analyzer::db::MirDbStorage) -> R,
    ) -> Option<R> {
        use std::panic::AssertUnwindSafe;
        let _interactive = self.interactive_read_guard();
        let session = self.analysis_session(self.workspace_php_version());
        for _ in 0..attempts {
            let db = session.snapshot_db();
            match salsa::Cancelled::catch(AssertUnwindSafe(|| f(&db))) {
                Ok(r) => return Some(r),
                Err(_) => drop(db),
            }
        }
        None
    }

    /// Mirror a file's current text into the salsa layer. Creates the
    /// `FileText` input on first sight, otherwise updates `text` on the
    /// existing input (bumping the salsa revision so downstream queries
    /// invalidate).
    pub fn mirror_text(&self, uri: &Url, text: &str) {
        // G2 fast path: compare against the lock-free text cache. When the
        // new text byte-matches what we already mirrored, skip the host
        // mutex entirely. Common during workspace scan + `did_open` for
        // unchanged files, where most threads would otherwise serialise on
        // `host.lock()` just to confirm a no-op.
        if let Some(cached) = self.caches.text_cache.get(uri)
            && **cached == *text
            && !self.deleted_uris.contains(uri)
            && self.lsp_ws_files.contains_key(uri)
        {
            return;
        }
        self.mirror_text_arc(uri, Arc::from(text))
    }

    /// Like [`mirror_text`] but takes an already-allocated `Arc<str>`.
    ///
    /// Callers that already hold an `Arc<str>` (e.g. `ingest_from_doc` reusing
    /// `ParsedDoc::source_arc()`) use this to avoid a second allocation and to
    /// ensure `text_cache` and `parsed_cache` hold the same Arc pointer —
    /// enabling `Arc::ptr_eq` validation in `get_parsed_cached`.
    pub fn mirror_text_arc(&self, uri: &Url, text_arc: Arc<str>) {
        let dur = Self::input_durability(uri);
        let path: Arc<str> = Arc::from(uri.as_str());
        let session = self.analysis_session(self.workspace_php_version());
        if let Some(wf) = self.lsp_ws_files.get(uri).map(|e| *e) {
            // A resurrected (previously-deleted) file changes
            // `workspace_file_paths()`'s result set — invalidate its cache.
            // The common case (an edit to an already-live file) hits `None`
            // here and pays nothing extra.
            if self.deleted_uris.remove(uri).is_some() {
                self.workspace_file_paths_cache.store(None);
            }
            // Fast path: byte-identical text already mirrored — skip the write
            // lock and the revision bump entirely.
            if let Some(cached) = self.caches.text_cache.get(uri)
                && **cached == *text_arc
            {
                return;
            }
            session.with_db_mut(|db| {
                let sf = wf.source(db);
                sf.set_text(db).with_durability(dur).to(text_arc.clone());
                // Any text change invalidates a previously-seeded cached index.
                // Only set when present to avoid a spurious second revision bump.
                if wf.cached_index(db).is_some() {
                    wf.set_cached_index(db).to(None);
                }
            });
            self.caches.text_cache.insert(uri.clone(), text_arc);
            // Evict only this file's analysis; cross-file invalidation is handled
            // lazily in `cached_analysis` via the declaration fingerprint.
            self.caches.evict_analysis(uri);
            self.write_revision.fetch_add(1, Ordering::Release);
        } else {
            let wf = session.with_db_mut(|db| {
                let sf = db.upsert_source_file_with_durability(path, text_arc.clone(), dur);
                LspWsFile::new(db, sf, None)
            });
            self.lsp_ws_files.insert(uri.clone(), wf);
            self.caches.text_cache.insert(uri.clone(), text_arc);
            self.mark_workspace_files_dirty();
            self.write_revision.fetch_add(1, Ordering::Release);
        }
    }

    /// Return the `LspWsFile` handle for a URL, if active (not deleted).
    #[cfg(test)]
    pub fn source_file(&self, uri: &Url) -> Option<LspWsFile> {
        if self.deleted_uris.contains(uri) {
            return None;
        }
        self.lsp_ws_files.get(uri).map(|e| *e)
    }

    /// Phase K2: pre-seed a `FileIndex` loaded from the on-disk cache onto
    /// the `FileText` input for `uri`. The next `file_index` call for that
    /// file returns the cached index directly, skipping parse + extract.
    ///
    /// Must be called **before** any `file_index(db, sf)` call for this file —
    /// otherwise salsa has already memoized the fresh-parse result and setting
    /// `cached_index` now would only bump the revision without using the cache.
    /// In practice the workspace-scan path seeds immediately after `mirror_text`
    /// and before any query runs.
    ///
    /// Returns `false` when `uri` was not mirrored (caller should mirror
    /// first); returns `true` on success.
    pub fn seed_cached_index(&self, uri: &Url, index: Arc<FileIndex>) -> bool {
        let Some(wf) = self.lsp_ws_file(uri) else {
            return false;
        };
        let session = self.analysis_session(self.workspace_php_version());
        session.with_db_mut(|db| wf.set_cached_index(db).to(Some(index)));
        true
    }

    /// Evict the semantic-tokens cache for `uri`. Called by Backend when a
    /// file is closed; diff-based tokens computed against the old revision
    /// are no longer meaningful.
    pub fn evict_token_cache(&self, uri: &Url) {
        self.caches.evict_tokens(uri);
    }

    /// Return the `FileIndex` for `uri` by running `file_index` on a salsa
    /// snapshot.  Returns `None` when `uri` has not been mirrored.
    ///
    /// Test-only — production code uses the salsa query directly via
    /// `snapshot_query`.
    #[cfg(test)]
    pub fn source_files_len(&self) -> usize {
        self.lsp_ws_files.len()
    }

    #[cfg(test)]
    pub fn snapshot_query_file_index(
        &self,
        uri: &Url,
    ) -> Option<crate::index::file_index::FileIndex> {
        let wf = self.lsp_ws_file(uri)?;
        Some(
            self.snapshot_mir_query(move |db| {
                (*crate::db::mir_queries::file_index(db, wf).0).clone()
            }),
        )
    }

    /// Register a file in the salsa layer without marking it open.
    ///
    /// Salsa's `parsed_doc` query parses lazily on first read; diagnostics
    /// are populated by `did_open` when the editor actually opens the file.
    pub fn ingest(&self, uri: Url, text: &str) {
        self.mirror_text(&uri, text);
    }

    /// Index a file using an already-parsed `ParsedDoc`, avoiding a second parse.
    ///
    /// Prefer this over [`ingest`] when the caller already has a `ParsedDoc` (e.g.
    /// after running `DefinitionCollector` during workspace scan). Reuses the
    /// `Arc<str>` already owned by `doc` so that `text_cache` and `SourceFile::text`
    /// share the same pointer — enabling the `Arc::ptr_eq` fast path in
    /// `get_parsed_cached` on the first subsequent salsa query, without an extra
    /// `Arc::from(source)` allocation.
    pub fn ingest_from_doc(&self, uri: Url, doc: &ParsedDoc) {
        self.mirror_text_arc(&uri, doc.source_arc());
    }

    pub fn remove(&self, uri: &Url) {
        self.caches.evict(uri);
        // Mark the URI as deleted but keep the `lsp_ws_files` entry so the
        // salsa `LspWsFile`/`SourceFile` handles remain alive. Re-opening the
        // file reuses the same handle instead of calling `LspWsFile::new()`
        // again, which would create a new orphaned salsa input on every
        // delete-reopen cycle. `session.invalidate_file` below already frees
        // the (potentially large) text this handle holds via mir's own
        // `remove_source_file`, so keeping the handle costs only its own
        // small footprint, not the file's content.
        self.deleted_uris.insert(uri.clone());
        self.mark_workspace_files_dirty();
        // Sync workspace files so the deleted file is removed from the salsa
        // `Workspace::files` list and won't appear in workspace symbols etc.
        self.sync_workspace_files();
        // Also evict the file from the `AnalysisSession`'s internal state so
        // workspace symbol queries don't keep returning the deleted file's
        // declarations. Cheap when the session hasn't ingested this file.
        let guard = self.analysis_session.lock().unwrap();
        if let Some((_, session)) = guard.as_ref() {
            session.invalidate_file(uri.as_str());
            // `file_index` has no LRU cap (unlike `parsed_doc`/`symbol_map`),
            // so without this it would keep holding this file's pre-deletion
            // `FileIndex` (and any seeded on-disk `cached_index`) in memory
            // for the rest of the process's life. Clear the seed and force
            // one recompute against the now-emptied text (`invalidate_file`
            // above already cleared it via mir's `remove_source_file`) to
            // shrink the memo to near-nothing immediately.
            if let Some(wf) = self.lsp_ws_files.get(uri).map(|e| *e) {
                session.with_db_mut(|db| {
                    if wf.cached_index(db).is_some() {
                        wf.set_cached_index(db).to(None);
                    }
                });
                let db = session.snapshot_db();
                let _ = crate::db::mir_queries::file_index(&db, wf);
            }
        }
        self.write_revision.fetch_add(1, Ordering::Release);
    }

    // ── Salsa-backed accessors ─────────────────────────────────────────────
    //
    // Reads run the memoized `parsed_doc` / `file_index` queries, parsing
    // only on first access per revision. These are the production accessors
    // used by every handler.

    /// Salsa-backed parsed document.
    ///
    /// Salsa-backed parsed document for any mirrored file (open or
    /// background-indexed). Returns `None` only when the file is not known
    /// to the store. Callers that want "only if open" should gate on
    /// `Backend::open_files` at the call site (see `Backend::get_doc`).
    pub fn get_doc_salsa(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        self.get_parsed_cached(uri)
    }

    /// Salsa-backed compact symbol index.
    pub fn get_index_salsa(&self, uri: &Url) -> Option<Arc<FileIndex>> {
        let wf = self.lsp_ws_file(uri)?;
        Some(
            self.snapshot_mir_query(move |db| crate::db::mir_queries::file_index(db, wf).0.clone()),
        )
    }

    /// Salsa-backed pre-computed symbol map (name → Vec<SymbolEntry>).
    /// Memoized per revision: stable files serve from cache in O(1).
    pub fn get_symbol_map_salsa(
        &self,
        uri: &Url,
    ) -> Option<Arc<crate::types::symbol_map::SymbolMap>> {
        // Symbol map runs on the shared mir db, sharing its memoized `parsed_doc`.
        let wf = self.lsp_ws_file(uri)?;
        Some(self.snapshot_mir_query(move |db| {
            let sf = *wf.source(db);
            crate::db::mir_queries::symbol_map(db, sf).0.clone()
        }))
    }

    /// Pre-computed symbol maps for every entry in `open_urls` except `uri`.
    pub fn other_symbol_maps(
        &self,
        uri: &Url,
        open_urls: &[Url],
    ) -> Vec<(Url, Arc<crate::types::symbol_map::SymbolMap>)> {
        open_urls
            .iter()
            .filter(|u| *u != uri)
            .filter_map(|u| self.get_symbol_map_salsa(u).map(|m| (u.clone(), m)))
            .collect()
    }

    /// G3: shared implementation for `get_doc_salsa`.
    /// Tries the `parsed_cache` (lock-free) first; validates via
    /// `Arc::ptr_eq` against the G2 `text_cache` so a concurrent writer
    /// that has already committed a new text input cannot be masked by a
    /// stale cache entry. On miss, captures the text Arc and ParsedDoc
    /// together inside a single `snapshot_query`, then publishes both.
    fn get_parsed_cached(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        if let Some(current_text) = self.caches.text_cache.get(uri)
            && let Some(entry) = self.caches.parsed_cache.get(uri)
            && Arc::ptr_eq(&*current_text, &entry.0)
        {
            let doc = entry.1.clone();
            drop(entry);
            self.caches.touch(uri);
            return Some(doc);
        }

        // Parse runs on the shared mir db.
        let wf = self.lsp_ws_file(uri)?;
        self.caches.bump_parse_count();
        let (text, doc) = self.snapshot_mir_query(move |db| {
            let sf = *wf.source(db);
            let text = sf.text(db).clone();
            let doc = crate::db::mir_queries::parsed_doc(db, sf).0.clone();
            (text, doc)
        });
        self.caches.insert_parsed(uri.clone(), text, doc.clone());
        Some(doc)
    }

    /// Parsed doc for a cosmetic, cursor-triggered read (e.g. `documentHighlight`)
    /// that must stay responsive while the user types. Unlike [`Self::get_doc_salsa`]
    /// it never spins on the `Cancelled` retry loop: it tries the lock-free
    /// `parsed_cache`, then a bounded snapshot parse, and if a concurrent write
    /// stream keeps cancelling it, returns the last-good cached `ParsedDoc`
    /// (possibly one edit stale — invisible for a highlight, unlike a hang).
    /// `None` only when the file has never been parsed.
    pub fn get_doc_snapshot_or_stale(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        if let Some(current_text) = self.caches.text_cache.get(uri)
            && let Some(entry) = self.caches.parsed_cache.get(uri)
            && Arc::ptr_eq(&*current_text, &entry.0)
        {
            let doc = entry.1.clone();
            drop(entry);
            self.caches.touch(uri);
            return Some(doc);
        }

        if let Some(wf) = self.lsp_ws_file(uri)
            && let Some((text, doc)) = self.try_snapshot_mir_query(3, move |db| {
                let sf = *wf.source(db);
                let text = sf.text(db).clone();
                let doc = crate::db::mir_queries::parsed_doc(db, sf).0.clone();
                (text, doc)
            })
        {
            self.caches.bump_parse_count();
            self.caches.insert_parsed(uri.clone(), text, doc.clone());
            return Some(doc);
        }

        self.caches.parsed_cache.get(uri).map(|e| e.1.clone())
    }

    /// Refresh `workspace.files` to mirror the current active file set.
    ///
    /// Skips all work when `workspace_files_dirty` is `false` (the common
    /// case after the workspace scan completes — file-set changes are rare).
    pub fn sync_workspace_files(&self) {
        // Atomically clear the flag.  If it was already false the file set
        // hasn't changed since the last sync; nothing to do.
        if !self.workspace_files_dirty.swap(false, Ordering::AcqRel) {
            return;
        }

        // Collect active (non-deleted) files, sorted by URI for stable ordering.
        let mut entries: Vec<(Arc<str>, LspWsFile)> = self
            .lsp_ws_files
            .iter()
            .filter(|e| !self.deleted_uris.contains(e.key()))
            .map(|e| (Arc::<str>::from(e.key().as_str()), *e.value()))
            .collect();
        entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        let files: Arc<[LspWsFile]> = entries.iter().map(|(_, wf)| *wf).collect();

        let session = self.analysis_session(self.workspace_php_version());
        let mut guard = self.lsp_workspace.lock().unwrap();
        session.with_db_mut(|db| match *guard {
            Some(ws) => {
                ws.set_files(db).to(files);
            }
            None => *guard = Some(LspWorkspace::new(db, files)),
        });
    }

    /// Mark the workspace file set as dirty so the next `sync_workspace_files`
    /// call re-runs the collect/sort/compare path, and invalidate the
    /// `workspace_file_paths()` cache. Exposed for benchmarks that need to
    /// measure the dirty-path cost in isolation.
    pub fn mark_workspace_files_dirty(&self) {
        self.workspace_files_dirty.store(true, Ordering::Release);
        self.workspace_file_paths_cache.store(None);
    }

    /// Update the PHP version tracked by the workspace. Salsa will invalidate
    /// all `semantic_issues` queries so diagnostics are re-evaluated.
    /// Skips the setter when the version hasn't changed to avoid spurious
    /// query invalidation.
    pub fn set_php_version(&self, version: mir_analyzer::PhpVersion) {
        {
            let mut guard = self.php_version.lock().unwrap();
            if *guard == version {
                return;
            }
            *guard = version;
        }
        // Changing the version selects a different `AnalysisSession` (and thus
        // a different db). In practice the version is set once at init, before
        // any file is mirrored.
        self.drop_session_scoped_state();
    }

    /// Discard every piece of state scoped to the current `AnalysisSession`'s
    /// salsa db. MUST accompany anything that causes `analysis_session()` to
    /// hand out a different db (version change, cache-dir attach): the
    /// `LspWsFile` input handles in `lsp_ws_files` index into the *old* db's
    /// tables, and using one against a new db panics with a salsa slot-type
    /// mismatch. The workspace scan (or the next edit) re-mirrors files onto
    /// the new session.
    fn drop_session_scoped_state(&self) {
        self.lsp_ws_files.clear();
        *self.lsp_workspace.lock().unwrap() = None;
        self.mark_workspace_files_dirty();
        // Cached FileAnalysis values reference the old session's state.
        self.caches.evict_analysis_all();
    }

    /// Inverted-index reference lookup scoped to `files` (the text-pre-filtered
    /// candidate set): a posting-list read for committed-fresh files plus an
    /// on-demand analyze+commit for stale/uncommitted candidates, so warm
    /// requests are O(results) instead of O(candidates).
    ///
    /// Returns LSP-style 0-based line/column.
    ///
    /// `cancel_rev`: when `Some(rev)`, the loop throws `salsa::Cancelled` and
    /// returns empty if a concurrent write advances the `write_revision` counter
    /// past `rev` — preventing unbounded retries against a newly-invalidated db.
    /// Pass `None` to retain the original indefinite-retry behaviour (fast ops
    /// like single-file reads where a stale result is not a concern).
    pub fn indexed_references(
        &self,
        symbol: &mir_analyzer::Name,
        files: &[Arc<str>],
        include_declaration: bool,
        cancel_rev: Option<u64>,
    ) -> Vec<(Arc<str>, u32, u32, u32)> {
        let php_version = self.workspace_php_version();
        // Staleness probe threaded into mir: polled at phase boundaries and
        // between cancellation retries, so a request invalidated by a
        // concurrent edit aborts *inside* mir's retry loop instead of spinning
        // there indefinitely (mir catches `Cancelled` internally, so an outer
        // catch alone never fires for the parallel phase).
        let stale =
            || cancel_rev.is_some_and(|rev| self.write_revision.load(Ordering::Acquire) != rev);
        // Retry: concurrent db writes (background indexing) cancel snapshot
        // queries via resume_unwind; without the loop the panic propagates out
        // of the caller's spawn_blocking and the request silently returns empty.
        let raw = loop {
            let session = self.analysis_session(php_version);
            match salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                session.indexed_references_to(symbol, files, include_declaration, &stale)
            })) {
                Ok(Some(refs)) => break refs,
                // mir aborted via the staleness probe — or a Phase-1 unwind
                // hit an already-stale request. Propagate as salsa::Cancelled
                // so spawn_blocking callers see a JoinError::Panicked and
                // return empty via unwrap_or_default.
                Ok(None) => {
                    std::panic::resume_unwind(Box::new(salsa::Cancelled::PendingWrite));
                }
                Err(_) if stale() => {
                    std::panic::resume_unwind(Box::new(salsa::Cancelled::PendingWrite));
                }
                // Phase-1 unwind from a write that doesn't invalidate this
                // request (mir-internal load_class, scan writes with no
                // cancel_rev) — retry.
                Err(_) => {}
            }
        };
        raw.into_iter()
            .map(|(file, range)| {
                // mir uses 1-based lines; 0-based columns (since mir 0.42.0).
                let line = range.start.line.saturating_sub(1);
                let col_start = range.start.column;
                let col_end = range.end.column;
                (file, line, col_start, col_end)
            })
            .collect()
    }

    /// `use`-import lines referencing `symbol`, from mir's `use:` postings.
    /// Read-only lookup with no freshness pass — run [`Self::indexed_references`]
    /// over the same `files` first so uncommitted candidates are analyzed.
    /// Returns `(file_uri, line, col_start, col_end)` tuples like
    /// [`Self::indexed_references`]; the range covers the whole import item.
    pub fn indexed_use_imports(
        &self,
        symbol: &mir_analyzer::Name,
        files: &[Arc<str>],
    ) -> Vec<(Arc<str>, u32, u32, u32)> {
        let session = self.analysis_session(self.workspace_php_version());
        let _interactive = self.interactive_read_guard();
        let raw = loop {
            if let Ok(locs) = salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                session.indexed_use_import_locations(symbol, files)
            })) {
                break locs;
            }
        };
        raw.into_iter()
            .map(|(file, range)| {
                // mir uses 1-based lines; 0-based columns.
                let line = range.start.line.saturating_sub(1);
                (file, line, range.start.column, range.end.column)
            })
            .collect()
    }

    /// Replay disk-cached reference postings and subtype edges for the whole
    /// mirrored workspace (mir's `warm_start_files`) — a no-op per file
    /// without a content-hash-matching cache entry from a previous run. Call
    /// once after the scan mirrors texts, before the analysis warm sweep, so
    /// a returning session starts index-warm.
    pub fn warm_start_indexes(&self) {
        let files: Vec<(Arc<str>, Arc<str>)> = self
            .lsp_ws_files
            .iter()
            .filter(|e| !self.deleted_uris.contains(e.key()))
            .filter_map(|e| {
                let text = self.caches.text_cache.get(e.key())?;
                Some((Arc::<str>::from(e.key().as_str()), Arc::clone(&text)))
            })
            .collect();
        if files.is_empty() {
            return;
        }
        let session = self.analysis_session(self.workspace_php_version());
        session.warm_start_files(&files);
    }

    /// Candidate file scope for a posting lookup on `symbol`.
    /// Private/protected methods narrow to their visibility scope; everything
    /// else gets the whole workspace. mir gates never-committed candidates on
    /// a symbol-name text mention internally (with PHP's case-insensitive
    /// matching semantics), so a host-side text prefilter would only re-scan
    /// the same bytes with weaker semantics.
    pub(crate) fn reference_candidate_files(&self, symbol: &mir_analyzer::Name) -> Vec<Arc<str>> {
        match symbol {
            mir_analyzer::Name::Method { class, name } => {
                if let Some(urls) = self.method_reference_scope(class, name) {
                    return urls.iter().map(|u| Arc::from(u.as_str())).collect();
                }
            }
            // A class reference always textually resolves the name in the
            // referencing file (import, namespace, or qualified path) —
            // there's no instance-typed-receiver indirection here, so FQN
            // narrowing is always sound.
            mir_analyzer::Name::Class(fqcn) => {
                if let Some(files) = self.fqn_reachable_files(std::slice::from_ref(fqcn)) {
                    return files;
                }
            }
            // Functions/constants resolve like class names ONLY when
            // namespaced: an unqualified call to a *global* one (`env()`,
            // `PHP_EOL`) works from any namespace via PHP's global
            // fallback, so no file can be excluded for those.
            mir_analyzer::Name::Function(fqcn) | mir_analyzer::Name::GlobalConstant(fqcn) => {
                if fqcn.trim_start_matches('\\').contains('\\')
                    && let Some(files) = self.fqn_reachable_files(std::slice::from_ref(fqcn))
                {
                    return files;
                }
            }
            _ => {}
        }
        self.workspace_file_paths().to_vec()
    }

    /// Every active workspace file as a mir path (`Arc<str>` of the URI).
    /// The candidate scope handed to mir's queries — mir gates uncommitted
    /// candidates on symbol-name mention internally, so this is always safe.
    ///
    /// Cached behind an `Arc` (invalidated by `mark_workspace_files_dirty`) so
    /// the ~15K-file walk + `Arc<str>` allocation per file only happens once
    /// per workspace-file-set change, not once per caller per request.
    pub fn workspace_file_paths(&self) -> Arc<Vec<Arc<str>>> {
        if let Some(cached) = self.workspace_file_paths_cache.load_full() {
            return cached;
        }
        let files: Arc<Vec<Arc<str>>> = Arc::new(
            self.lsp_ws_files
                .iter()
                .filter(|e| !self.deleted_uris.contains(e.key()))
                .map(|e| Arc::<str>::from(e.key().as_str()))
                .collect(),
        );
        self.workspace_file_paths_cache.store(Some(Arc::clone(&files)));
        files
    }

    /// Transitive subtypes of `class_fqn` from mir's maintained subtype edge
    /// index, with declaration sites. `include_trait_users` also counts
    /// `use Trait;` composition as a subtype edge (trait-usage lenses); leave
    /// it off for goto-implementation semantics (extends/implements only).
    pub fn indexed_subtype_classes(
        &self,
        class_fqn: &str,
        include_trait_users: bool,
    ) -> Vec<mir_analyzer::SubtypeClassSite> {
        let files = self.workspace_file_paths();
        let session = self.analysis_session(self.workspace_php_version());
        let _interactive = self.interactive_read_guard();
        loop {
            if let Ok(sites) = salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                session.indexed_subtype_classes(class_fqn, &files, include_trait_users)
            })) {
                break sites;
            }
        }
    }

    /// Concrete implementations of `class_fqn::method` across its subtypes:
    /// `(subtype fqcn, file, name range)` from mir's subtype edge index.
    pub fn indexed_method_implementations(
        &self,
        class_fqn: &str,
        method: &str,
    ) -> Vec<(Arc<str>, Arc<str>, mir_analyzer::Range)> {
        let files = self.workspace_file_paths();
        let session = self.analysis_session(self.workspace_php_version());
        let _interactive = self.interactive_read_guard();
        loop {
            if let Ok(sites) = salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                session.indexed_method_implementations(class_fqn, method, &files)
            })) {
                break sites;
            }
        }
    }

    /// The complete set of files a `private` method's references can occur in,
    /// or `None` when the search must stay workspace-wide.
    ///
    /// `owner_fqn` is the enclosing class at the cursor; narrowing applies only
    /// when that class *directly declares* `method` (so its `MethodDef`
    /// visibility is authoritative — inherited/call-site lookups fall through to
    /// `None`). A reference to `Class::method` is recorded only where
    /// `analyze_file` resolves an expression to that key, and `analyze_file` is
    /// per-source-file, so a `private` member — resolvable only inside the
    /// declaring class body, which lives in one file — is fully scoped to the
    /// declaring file at any indexing stage.
    ///
    /// A `protected` member is reachable from the declaring class and its
    /// transitive subclasses (`$this->m()`, `parent::m()`, `self::`/`static::`),
    /// all of which live in the declaring file or a subtype file — never outside
    /// the hierarchy in valid PHP. Its scope is therefore the declaring file plus
    /// the transitive subtype files. The complete subtype set is only known once
    /// the workspace reference index finishes, so this narrowing applies only
    /// when [`Self::is_index_ready`]; before then it falls back to `None` (full
    /// workspace scope) so references are never under-reported.
    ///
    /// Classes that compose traits or mixins are excluded — those inline
    /// external resolution context, so a reference could surface in another file.
    pub fn method_reference_scope(&self, owner_fqn: &str, method: &str) -> Option<Vec<Url>> {
        use crate::index::file_index::{ClassKind, Visibility};

        let ws = self.get_workspace_index_salsa();
        let owner_fqn = owner_fqn.trim_start_matches('\\');
        let owner_short = crate::text::fqn_short_name(owner_fqn);

        let (decl_uri, decl_class) = ws
            .classes_by_name
            .get(owner_short)?
            .iter()
            .filter_map(|&r| ws.at(r))
            .find(|(_, cls)| cls.fqn.trim_start_matches('\\') == owner_fqn)?;

        // Trait/interface methods, and methods on classes that compose traits or
        // mixins, can be referenced from other files — keep the full scope.
        if !matches!(decl_class.kind, ClassKind::Class | ClassKind::Enum)
            || !decl_class.traits.is_empty()
            || !decl_class.mixins.is_empty()
        {
            return None;
        }

        let method_def = decl_class
            .methods
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(method))?;

        match method_def.visibility {
            Visibility::Private => Some(vec![decl_uri.clone()]),
            Visibility::Protected => {
                if !self.is_index_ready() {
                    return None;
                }
                // Subtype set comes from mir's resolved inheritance graph — it
                // matches subclasses by FQCN, so `extends \Ns\Base` and aliased
                // `use ... as` forms are all found. Falls back to full scope if
                // mir can't resolve the owner.
                let session = self.analysis_session(self.workspace_php_version());
                let mut files: std::collections::HashSet<Url> = session
                    .subtype_files(owner_fqn)
                    .into_iter()
                    .filter_map(|p| Url::parse(&p).ok())
                    .collect();
                files.insert(decl_uri.clone());
                Some(files.into_iter().collect())
            }
            // A public *static* method's call sites come in exactly two
            // textual shapes: a resolved class name (`Owner::m()`, inherited
            // `Sub::m()`, `self::`/`static::`/`parent::m()` from inside the
            // hierarchy, aliased `use Owner as X; X::m()` — all covered by
            // owner+subtype FQN reachability) or an *instance* receiver
            // (`$obj::m()`, whose file may never name the class but always
            // contains the member token — covered by the member-name
            // needle). Dynamic member names (`Owner::$m()`) produce no
            // posting at all (verified), so the union is exhaustive. This
            // beats mir's own gate, which only sees bare short names and
            // can't tell this owner apart from an unrelated class sharing
            // its short name. Instance methods stay unnarrowed (`None`
            // below): a typed receiver can hide both the class *and* any
            // distinguishing token cheaper than the member name itself.
            Visibility::Public
                if method_def.is_static || method.eq_ignore_ascii_case("__construct") =>
            {
                if !self.is_index_ready() {
                    return None;
                }
                let mut fqns: Vec<Arc<str>> = vec![Arc::from(owner_fqn)];
                fqns.extend(
                    self.indexed_subtype_classes(owner_fqn, false)
                        .into_iter()
                        .map(|s| s.fqcn),
                );
                // `__construct`'s extra needle is the call-shaped
                // `->__construct` (explicit re-init on a typed receiver),
                // not the bare identifier — that would drag in every file
                // *declaring* a constructor. `new X` sites and
                // `parent::`/`self::`/`static::__construct()` calls need no
                // needle: the former textually resolve an owner/subtype
                // name, the latter live inside hierarchy files, which the
                // subtype closure's namespace rule already admits (unlike
                // mir's own gate, which lacks the closure and keeps a
                // `::__construct` token for the grandchild-`parent::` case).
                let files = if method.eq_ignore_ascii_case("__construct") {
                    self.fqn_reachable_files_with_needles(&fqns, &["->__construct"])?
                } else {
                    self.fqn_reachable_files_with_needles(&fqns, &[method])?
                };
                Some(files.iter().filter_map(|f| Url::parse(f).ok()).collect())
            }
            Visibility::Public => None,
        }
    }

    /// Files that could possibly reference any FQN in `fqns`, narrowed via
    /// PHP's own name-resolution rules rather than bare-word text mention:
    ///
    /// 1. **Namespace rule** — the file's namespace equals the target's, or
    ///    is a `\`-segment prefix of it: `namespace Foo;` reaches
    ///    `Foo\Bar\Baz` through the relative-qualified `Bar\Baz`.
    /// 2. **Import rule** — a `use` of the target itself, an alias of it, or
    ///    any namespace prefix of it (`use Foo\Bar; Bar\Baz::x()`), all
    ///    matched on the import's FQN so aliases come for free.
    /// 3. **Text rule** — the qualified path appears literally in the file
    ///    (`\App\Widget::class` config arrays with no `use` line, or a
    ///    leading-slash-free `App\Widget` from a no-namespace file, where
    ///    qualified names resolve from the root). Matched
    ///    ASCII-case-insensitively — PHP class names are case-insensitive.
    ///
    /// These are the *only* ways PHP resolves a class-like or namespaced
    /// name, so this is exact narrowing grounded in the language's own
    /// resolution order, not a heuristic — unlike mir's own gate, which
    /// only sees bare short names and can't distinguish this owner class
    /// from an unrelated class sharing its short name elsewhere in the
    /// workspace.
    ///
    /// Callers must NOT use this for global-namespace functions/constants
    /// (unqualified calls fall back to the global namespace from *any*
    /// namespace, so no file can be excluded) or for instance members (a
    /// typed receiver can hide the class entirely).
    ///
    /// Returns `None` (full workspace) when the boot scan hasn't finished —
    /// same conservative discipline as the `Protected` branch above — so
    /// references are never under-reported at cold start.
    pub fn fqn_reachable_files(&self, fqns: &[Arc<str>]) -> Option<Vec<Arc<str>>> {
        self.fqn_reachable_files_with_needles(fqns, &[])
    }

    /// [`Self::fqn_reachable_files`] plus files whose text contains any of
    /// `extra_needles` (ASCII-case-insensitive substring, no word bounds).
    /// The union is what makes member-symbol narrowing sound: a static call
    /// through an *instance* receiver (`$obj::make()`) or an explicit
    /// re-init (`$obj->__construct()`) references a member without the file
    /// ever resolving the owner's name, but its text always contains the
    /// call token itself.
    fn fqn_reachable_files_with_needles(
        &self,
        fqns: &[Arc<str>],
        extra_needles: &[&str],
    ) -> Option<Vec<Arc<str>>> {
        use rayon::prelude::*;

        if !self.is_index_ready() {
            return None;
        }
        let targets: Vec<&str> = fqns.iter().map(|f| f.trim_start_matches('\\')).collect();
        let target_namespaces: Vec<Option<&str>> = targets
            .iter()
            .map(|t| t.rsplit_once('\\').map(|(ns, _)| ns))
            .collect();
        // Namespaced targets match with or without the leading `\`; a bare
        // global name keeps it (the short name alone would match everything).
        let needles: Vec<String> = targets
            .iter()
            .map(|t| {
                if t.contains('\\') {
                    (*t).to_string()
                } else {
                    format!("\\{t}")
                }
            })
            .chain(extra_needles.iter().map(|n| (*n).to_string()))
            .collect();
        let finder = aho_corasick::AhoCorasick::builder()
            .ascii_case_insensitive(true)
            .build(&needles)
            .ok()?;

        let ws = self.get_workspace_index_salsa();
        // The namespace/import checks are cheap in-memory field comparisons,
        // but the qualified-mention fallback scans each candidate's full
        // text — for a common short name, most of the corpus fails the
        // cheap checks and falls through to it, so this must run in
        // parallel (mir's own equivalent gate scan does the same) or a
        // cold query on a 15K-file workspace pays a multi-second sequential
        // scan instead of the few-ms parallel one.
        Some(
            ws.files
                .par_iter()
                .filter(|(url, idx)| {
                    let file_ns = idx.namespace.as_deref().map(|s| s.trim_start_matches('\\'));
                    let ns_match = target_namespaces.iter().any(|ns| match (file_ns, ns) {
                        (Some(f), Some(t)) => fqn_segment_prefix(f, t),
                        (None, None) => true,
                        // Global-ns files reach namespaced targets only via
                        // the full path (text rule); namespaced files reach
                        // global classes only via `\Name` or an import.
                        _ => false,
                    });
                    if ns_match {
                        return true;
                    }
                    let import_match = idx.use_imports.iter().any(|(_, f)| {
                        let f = f.trim_start_matches('\\');
                        targets.iter().any(|t| fqn_segment_prefix(f, t))
                    });
                    if import_match {
                        return true;
                    }
                    self.source_text(url)
                        .is_some_and(|text| finder.is_match(text.as_ref()))
                })
                .map(|(u, _)| Arc::<str>::from(u.as_str()))
                .collect(),
        )
    }

    /// Phase J: salsa-memoized aggregate workspace index.
    ///
    /// Returns the shared `Arc<WorkspaceIndexData>` with flat
    /// `(Url, Arc<FileIndex>)` list plus pre-built `classes_by_name` and
    /// `subtypes_of` reverse maps. Used by workspace_symbols,
    /// prepare_type_hierarchy, supertypes_of, subtypes_of, and
    /// find_implementations so they don't each rebuild the aggregate per
    /// request. Invalidates automatically when any file's `file_index`
    /// changes.
    pub fn get_workspace_index_salsa(&self) -> Arc<crate::db::workspace_index::WorkspaceIndexData> {
        self.sync_workspace_files();
        let ws = *self.lsp_workspace.lock().unwrap();
        let Some(ws) = ws else {
            return Arc::new(crate::db::workspace_index::WorkspaceIndexData {
                files: Vec::new(),
                classes_by_name: std::collections::HashMap::new(),
                subtypes_of: std::collections::HashMap::new(),
                decls_by_name: std::collections::HashMap::new(),
                classes_by_lowercase_name: Vec::new(),
            });
        };
        self.snapshot_mir_query(move |db| crate::db::mir_queries::workspace_index(db, ws).0.clone())
    }

    /// Total number of real `ParsedDoc` parses served so far (cache misses).
    /// Surfaced via `$/php-lsp/debugStats` so tests can assert the references
    /// read path doesn't parse the whole workspace.
    pub fn parse_count(&self) -> u64 {
        self.caches.parse_count()
    }

    /// Times mir's `RefIndex` was locked on the current session. Reads are
    /// per-key posting lookups and edits commit one file's postings, so this
    /// grows by a small bounded amount per operation — never per candidate.
    /// Surfaced via `$/php-lsp/debugStats` for the stress-test guard.
    pub fn ref_index_lock_count(&self) -> u64 {
        self.analysis_session(self.workspace_php_version())
            .ref_index_lock_count()
    }

    /// Return the raw source text for `uri` if it has been mirrored into the
    /// salsa workspace. Used by the references handler to pre-filter session
    /// results by checking whether a file mentions the owning class name.
    pub fn source_text(&self, uri: &Url) -> Option<Arc<str>> {
        self.caches.text_cache.get(uri).map(|e| Arc::clone(&e))
    }

    /// Cache the semantic tokens computed for a delta response.
    /// `result_id` is an opaque string (a hash of the token data) returned to the client.
    pub fn store_token_cache(&self, uri: &Url, result_id: String, tokens: Arc<Vec<SemanticToken>>) {
        self.caches.store_token(uri, result_id, tokens);
    }

    /// Return the cached tokens if `result_id` matches the stored one.
    pub fn get_token_cache(&self, uri: &Url, result_id: &str) -> Option<Arc<Vec<SemanticToken>>> {
        self.caches.get_token(uri, result_id)
    }

    /// Raw semantic issues for a file, computed via mir's session-based
    /// `FileAnalyzer`. The session lazy-loads dependencies via PSR-4 so the
    /// LSP no longer needs to mirror vendor up-front. Callers apply their
    /// own `DiagnosticsConfig` filter via
    /// [`crate::semantic_diagnostics::issues_to_diagnostics`].
    #[tracing::instrument(skip_all)]
    pub fn get_semantic_issues_salsa(&self, uri: &Url) -> Option<Arc<[mir_issues::Issue]>> {
        let analysis = self.cached_analysis(uri)?;
        let file: Arc<str> = Arc::from(uri.as_str());
        // Workspace-level class issues for this file (circular inheritance,
        // override violations, abstract-method gaps). These are session-wide
        // (a dependency edit changes them without changing this file's bytes),
        // so they are recomputed live rather than cached alongside the
        // per-file body analysis.
        let class_issues = {
            let _s = tracing::debug_span!("session.class_issues_for").entered();
            // Retry: concurrent db writes cancel snapshot queries via resume_unwind.
            loop {
                let session = self.analysis_session(self.workspace_php_version());
                if let Ok(issues) = salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                    session.class_issues(std::slice::from_ref(&file))
                })) {
                    break issues;
                }
            }
        };
        let combined: Vec<mir_issues::Issue> = analysis
            .issues
            .iter()
            .cloned()
            .chain(class_issues)
            .filter(|i| !i.suppressed)
            .collect();
        Some(Arc::from(combined))
    }

    /// Run (or reuse) mir's per-file body analysis, retaining the full
    /// [`mir_analyzer::FileAnalysis`] — issues **and** resolved symbols — across
    /// requests. Diagnostics read `.issues`; position features call
    /// `.symbol_at(offset)` for the resolved type at a cursor.
    ///
    /// Cache hit when the entry's captured source `Arc` is pointer-equal to the
    /// file's current `doc.source_arc()`. A miss recomputes and overwrites, so
    /// the entry self-evicts on any content edit.
    /// Cache-hit-only variant of [`Self::cached_analysis`]: returns the cached
    /// analysis when the entry is current for the file's text, never computes.
    /// Lets async handlers take the warm path synchronously and reserve
    /// `spawn_blocking` for the cold path (mir Pass 1 + Pass 2 can take
    /// hundreds of ms on large files).
    pub fn cached_analysis_if_fresh(&self, uri: &Url) -> Option<Arc<mir_analyzer::FileAnalysis>> {
        let doc = self.get_doc_salsa(uri)?;
        let source = doc.source_arc();
        let cur_ver = self.caches.decl_version();
        let analysis = {
            let entry = self.caches.analysis_cache.get(uri)?;
            (Arc::ptr_eq(&entry.0, &source) && entry.1 == cur_ver).then(|| Arc::clone(&entry.2))?
        };
        self.caches.touch(uri);
        Some(analysis)
    }

    pub fn cached_analysis(&self, uri: &Url) -> Option<Arc<mir_analyzer::FileAnalysis>> {
        self.cached_analysis_cancellable(uri, &|| false)
    }

    /// [`Self::cached_analysis`] with an early exit: `should_cancel` is
    /// polled whenever a concurrent salsa write cancels the analysis attempt.
    /// A handler passes its write-revision staleness probe so a request made
    /// obsolete by newer typing stops burning a blocking-pool thread instead
    /// of retrying until the writer pauses — the editor re-requests anyway.
    #[tracing::instrument(skip_all)]
    pub fn cached_analysis_cancellable(
        &self,
        uri: &Url,
        should_cancel: &(dyn Fn() -> bool + Sync),
    ) -> Option<Arc<mir_analyzer::FileAnalysis>> {
        // Need the parsed doc both for the analyzer and as the cache key.
        let doc = self.get_doc_salsa(uri)?;
        let source = doc.source_arc();

        let cur_ver = self.caches.decl_version();
        if let Some(entry) = self.caches.analysis_cache.get(uri)
            && Arc::ptr_eq(&entry.0, &source)
            && entry.1 == cur_ver
        {
            let analysis = Arc::clone(&entry.2);
            drop(entry);
            self.caches.touch(uri);
            return Some(analysis);
        }

        let php_version = self.workspace_php_version();
        let session = self.analysis_session(php_version);
        let file: Arc<str> = Arc::from(uri.as_str());

        let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());
        let owned_program = if let Some(cached) = self.caches.owned_program_cache.get(uri)
            && Arc::ptr_eq(&cached.0, &source)
        {
            Arc::clone(&cached.1)
        } else {
            let prog = Arc::new(php_ast::owned::to_owned_program(doc.program()));
            self.caches.shed_stale(
                &self.caches.owned_program_cache,
                crate::document::cache_registry::OWNED_PROGRAM_CACHE_CAP,
            );
            self.caches
                .owned_program_cache
                .insert(uri.clone(), (Arc::clone(&source), Arc::clone(&prog)));
            prog
        };

        // autoload.files helpers (e.g. Laravel's tap()) must be ingested so mir sees their functions.
        let autoload_texts: Vec<(Arc<str>, Arc<str>)> = {
            let autoload_uris = self.autoload_uris.read().unwrap().clone();
            autoload_uris
                .iter()
                .filter_map(|auri| {
                    self.caches
                        .text_cache
                        .get(auri)
                        .map(|t| (Arc::from(auri.as_str()), Arc::clone(&*t)))
                })
                .collect()
        };
        // Bare same-namespace / use-imported class refs aren't resolved by mir's priority_index_for_ast; preload them.
        let class_fqns = crate::navigation::references::collect_referenced_class_fqns(&doc);

        // ingest_file/load_class/analyze take internal salsa snapshots; a concurrent db write cancels them via resume_unwind. Retry the idempotent sequence.
        let _interactive = self.interactive_read_guard();
        let analysis = loop {
            let attempt = salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                session.ingest_file(file.clone(), source.clone());
                for (afile, atext) in &autoload_texts {
                    session.ingest_file(afile.clone(), atext.clone());
                }
                for fqcn in &class_fqns {
                    let _ = session.load_class(fqcn);
                }
                let analyzer = mir_analyzer::FileAnalyzer::new(&session);
                analyzer.analyze(file.clone(), doc.source(), &owned_program, &source_map)
            }));
            match attempt {
                Ok(a) => break Arc::new(a),
                Err(_) => {
                    // A write cancelled the attempt. If it replaced THIS
                    // file's text the result is already obsolete (the cache
                    // key is the source Arc) and the editor re-requests after
                    // its didChange — stop burning the blocking thread.
                    // Writes elsewhere just retry as before.
                    let text_changed = self
                        .caches
                        .text_cache
                        .get(uri)
                        .is_none_or(|t| !Arc::ptr_eq(&t, &source));
                    if text_changed || should_cancel() {
                        return None;
                    }
                }
            }
        };
        // Compare the new FileIndex against the stored fingerprint. If
        // declarations changed (or this is the first analysis), bump
        // `decl_version` so other files' cache entries become stale. Body-only
        // edits leave the counter unchanged, allowing sibling files to be
        // served from cache on the next request.
        let new_index = self.get_index_salsa(uri);
        let old_fp = self
            .caches
            .decl_fingerprints
            .get(uri)
            .map(|e| Arc::clone(&*e));
        let decl_changed = match (&old_fp, &new_index) {
            (Some(old), Some(new)) => **old != **new,
            // First analysis: only a file that actually declares something can
            // affect other files. Opening a plain script must not invalidate
            // every open file's analysis cache and mir's warm-up marks.
            (None, Some(new)) => !new.declares_nothing(),
            _ => false,
        };
        if decl_changed {
            if let Some(idx) = new_index {
                self.caches.decl_fingerprints.insert(uri.clone(), idx);
            }
            self.caches.bump_decl_version();
            // Text reaches mir via direct salsa `set_text` writes, so mir can't
            // see declaration deletions itself. A deleted declaration may
            // unshadow a lazy-loadable symbol — invalidate mir's warm-up skip
            // set so reference queries re-run their prepare pass once.
            session.bump_prepare_generation();
        }
        // Tag with the version observed before the analysis ran, plus this
        // file's own bump. Reading `decl_version()` here instead would absorb
        // a concurrent bump from another file's mid-compute declaration change
        // into the tag, marking a possibly-stale result as fresh. The
        // conservative tag errs toward one extra recompute, never staleness.
        let ver = cur_ver + u64::from(decl_changed);
        self.caches.shed_stale(
            &self.caches.analysis_cache,
            crate::document::cache_registry::ANALYSIS_CACHE_CAP,
        );
        self.caches.touch(uri);
        self.caches
            .analysis_cache
            .insert(uri.clone(), (source, ver, Arc::clone(&analysis)));
        Some(analysis)
    }

    /// Returns `(uri, doc)` for files currently open in the editor.
    ///
    /// Resolve `open_urls` (from `Backend::open_urls()`) to parsed docs.
    /// Files not mirrored in the salsa layer are filtered out silently.
    pub fn docs_for(&self, open_urls: &[Url]) -> Vec<(Url, Arc<ParsedDoc>)> {
        open_urls
            .iter()
            .filter_map(|u| self.get_doc_salsa(u).map(|d| (u.clone(), d)))
            .collect()
    }

    /// Parsed docs for every entry in `open_urls` except `uri`.
    pub fn other_docs(&self, uri: &Url, open_urls: &[Url]) -> Vec<(Url, Arc<ParsedDoc>)> {
        open_urls
            .iter()
            .filter(|u| *u != uri)
            .filter_map(|u| self.get_doc_salsa(u).map(|d| (u.clone(), d)))
            .collect()
    }

    /// Compact symbol index for every mirrored file.
    pub fn all_indexes(&self) -> Vec<(Url, Arc<FileIndex>)> {
        self.get_workspace_index_salsa().files.clone()
    }

    /// Borrow-scoped alternative to `all_indexes()` for callers that only
    /// need the slice for the duration of one synchronous call — avoids
    /// cloning every `Url` in the aggregate (`get_workspace_index_salsa()`
    /// itself is a cheap `Arc` clone; `all_indexes()`'s `.files.clone()` is
    /// the expensive part). Use `all_indexes()` instead when the result must
    /// be moved across an `.await`/`spawn_blocking` boundary.
    pub fn with_all_indexes<R>(&self, f: impl FnOnce(&[(Url, Arc<FileIndex>)]) -> R) -> R {
        f(&self.get_workspace_index_salsa().files)
    }

    /// Store a lazily-loaded vendor `FileIndex` in the session cache.
    /// Only call this for files that are not part of the normal workspace scan
    /// (i.e. vendor files loaded on-demand by PSR-4 navigation).
    pub fn cache_vendor_index(&self, uri: Url, index: Arc<FileIndex>) {
        self.caches.vendor_index_cache.insert(uri, index);
    }

    /// Retrieve a previously cached vendor `FileIndex`.
    pub fn get_vendor_index(&self, uri: &Url) -> Option<Arc<FileIndex>> {
        self.caches
            .vendor_index_cache
            .get(uri)
            .map(|e| Arc::clone(&*e))
    }

    /// Same as `all_indexes` but excludes `uri`.
    pub fn other_indexes(&self, uri: &Url) -> Vec<(Url, Arc<FileIndex>)> {
        self.get_workspace_index_salsa()
            .files
            .iter()
            .filter(|(u, _)| u != uri)
            .cloned()
            .collect()
    }

    /// Parsed documents for every mirrored file (open or background-indexed).
    /// Suitable for full-scan operations: find-references, rename,
    /// call_hierarchy, code_lens.
    pub fn all_docs_for_scan(&self) -> Vec<(Url, Arc<ParsedDoc>)> {
        let urls: Vec<Url> = self
            .lsp_ws_files
            .iter()
            .filter(|e| !self.deleted_uris.contains(e.key()))
            .map(|e| e.key().clone())
            .collect();
        urls.into_iter()
            .filter_map(|u| self.get_doc_salsa(&u).map(|d| (u, d)))
            .collect()
    }

    /// Files whose `use` imports include `fqn` (leading `\` and ASCII case
    /// ignored — PHP names are case-insensitive), from the workspace symbol
    /// index — no parsing, no text scan. The candidate scope for `use`-line
    /// rewrites on file rename/delete: only importers can carry such a line.
    pub fn files_importing(&self, fqn: &str) -> Vec<Url> {
        let target = fqn.trim_start_matches('\\');
        self.get_workspace_index_salsa()
            .files
            .iter()
            .filter(|(_, idx)| {
                idx.use_imports
                    .iter()
                    .any(|(_, f)| f.trim_start_matches('\\').eq_ignore_ascii_case(target))
            })
            .map(|(u, _)| u.clone())
            .collect()
    }
}

/// `prefix` equals `whole` or is a `\`-segment-aligned prefix of it,
/// ASCII-case-insensitively (PHP name semantics). `"Foo\Bar"` is a segment
/// prefix of `"Foo\Bar\Baz"` but `"Foo\Ba"` is not.
fn fqn_segment_prefix(prefix: &str, whole: &str) -> bool {
    let (p, w) = (prefix.as_bytes(), whole.as_bytes());
    w.len() >= p.len()
        && w[..p.len()].eq_ignore_ascii_case(p)
        && (w.len() == p.len() || w[p.len()] == b'\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    /// Phase E4: open-file state lives on `Backend`, not `DocumentStore`.
    /// Tests that need to simulate "file is open" just mirror the text into
    /// the salsa input — the open/closed distinction is enforced by the
    /// caller (Backend) in production.
    fn open(store: &DocumentStore, u: Url, text: String) {
        store.mirror_text(&u, &text);
    }

    // Removed `salsa_codebase_aggregates_all_files`: the salsa-side codebase
    // aggregation was deleted with the mir 0.22 migration. Equivalent
    // behaviour is now covered by mir-analyzer's own session tests.

    // Spawn a thread that calls `yield_to_interactive_reads` and hands back
    // the elapsed time it spent waiting. It signals `ready` right before
    // entering the call, so the caller can block on that signal instead of
    // guessing how long thread start-up takes — a fixed sleep here would
    // race the thread scheduler, and that race is exactly what made this
    // test flaky under load (e.g. Windows CI runners have much coarser and
    // less predictable thread-spawn latency than Linux/macOS).
    fn spawn_yield_waiter(
        store: &Arc<DocumentStore>,
    ) -> (
        std::sync::mpsc::Receiver<()>,
        std::thread::JoinHandle<std::time::Duration>,
    ) {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let store = Arc::clone(store);
        let handle = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let t = std::time::Instant::now();
            store.yield_to_interactive_reads();
            t.elapsed()
        });
        (ready_rx, handle)
    }

    #[test]
    fn scan_writers_yield_while_interactive_reads_are_in_flight() {
        let store = Arc::new(DocumentStore::new());

        // No guard: returns immediately.
        let t = std::time::Instant::now();
        store.yield_to_interactive_reads();
        assert!(t.elapsed() < std::time::Duration::from_millis(200));

        // Guard held on another thread: the writer waits until it drops.
        // We only start the hold timer once the waiter has confirmed it's
        // about to poll, so the guard can never be dropped before the
        // writer starts waiting on it.
        let guard = store.interactive_read_guard();
        let (ready_rx, waiter) = spawn_yield_waiter(&store);
        ready_rx.recv().unwrap();
        const HOLD: std::time::Duration = std::time::Duration::from_millis(60);
        std::thread::sleep(HOLD);
        drop(guard);
        let waited = waiter.join().unwrap();
        assert!(
            waited >= HOLD,
            "writer should have paused at least as long as the read guard was held (waited {waited:?})"
        );

        // Guard fully released: nested guards count correctly.
        let g1 = store.interactive_read_guard();
        let g2 = store.interactive_read_guard();
        drop(g1);
        let (ready_rx, waiter) = spawn_yield_waiter(&store);
        ready_rx.recv().unwrap();
        const HOLD2: std::time::Duration = std::time::Duration::from_millis(30);
        std::thread::sleep(HOLD2);
        drop(g2);
        let waited = waiter.join().unwrap();
        assert!(
            waited >= HOLD2,
            "writer should have paused while the nested read guard was held (waited {waited:?})"
        );
        store.yield_to_interactive_reads(); // all guards dropped — no wait
    }

    #[test]
    fn index_registers_file_in_salsa() {
        let store = DocumentStore::new();
        store.ingest(uri("/lib.php"), "<?php\nfunction lib_fn() {}");
        let idx = store.get_index_salsa(&uri("/lib.php")).unwrap();
        assert_eq!(idx.functions.len(), 1);
        assert_eq!(&*idx.functions[0].name, "lib_fn");
    }

    #[test]
    fn remove_hides_file_from_index() {
        let store = DocumentStore::new();
        let u = uri("/lib.php");
        store.ingest(u.clone(), "<?php");
        store.remove(&u);
        assert!(store.get_index_salsa(&u).is_none());
    }

    #[test]
    fn remove_frees_mir_source_text() {
        let store = DocumentStore::new();
        let u = uri("/lib.php");
        let big_text = format!("<?php\n{}", "// pad\n".repeat(1000));
        store.ingest(u.clone(), &big_text);

        let session = store.analysis_session(store.workspace_php_version());
        let sf = session.lookup_source_file(u.as_str()).unwrap();
        let len_before = store.snapshot_mir_query(|db| sf.text(db).len());
        assert!(len_before > 1000, "sanity: source text should be mirrored");

        store.remove(&u);

        let len_after = store.snapshot_mir_query(|db| sf.text(db).len());
        assert_eq!(
            len_after, 0,
            "remove() must free mir's SourceFile text, not just hide the file from indexes"
        );
    }

    #[test]
    fn remove_shrinks_file_index_memo() {
        let store = DocumentStore::new();
        let u = uri("/lib.php");
        store.ingest(u.clone(), "<?php\nclass BigClassBeforeDelete {}\n");

        let wf = store.lsp_ws_files.get(&u).map(|e| *e).unwrap();
        let idx_before =
            store.snapshot_mir_query(|db| crate::db::mir_queries::file_index(db, wf).0.clone());
        assert_eq!(
            idx_before.classes.len(),
            1,
            "sanity: class should be indexed"
        );

        store.remove(&u);

        let idx_after =
            store.snapshot_mir_query(|db| crate::db::mir_queries::file_index(db, wf).0.clone());
        assert!(
            idx_after.classes.is_empty(),
            "remove() must shrink file_index's memo (no LRU cap, unlike parsed_doc/symbol_map) \
             against the emptied text, not hold the pre-deletion FileIndex forever"
        );
    }

    #[test]
    fn remove_and_reopen_reuses_source_file_handle() {
        let store = DocumentStore::new();
        let u = uri("/lib.php");
        store.ingest(u.clone(), "<?php");
        let ft_before = store.source_file(&u).unwrap();
        store.remove(&u);
        assert!(
            store.source_file(&u).is_none(),
            "deleted file should be hidden"
        );
        store.mirror_text(&u, "<?php");
        let ft_after = store.source_file(&u).unwrap();
        assert!(
            ft_before == ft_after,
            "reopen must reuse the same FileText handle"
        );
    }

    #[test]
    fn delete_reopen_churn_does_not_amplify_salsa_inputs() {
        let store = DocumentStore::new();
        let uris: Vec<Url> = (0..20).map(|i| uri(&format!("/churn/f{i}.php"))).collect();
        for u in &uris {
            store.ingest(u.clone(), "<?php class A {}");
        }
        let count_before = store.source_files_len();
        for _ in 0..10 {
            for u in &uris {
                store.remove(u);
            }
            for u in &uris {
                store.ingest(u.clone(), "<?php class A {}");
            }
        }
        assert_eq!(
            store.source_files_len(),
            count_before,
            "delete-reopen cycles must not create new salsa inputs (L1-B regression guard)"
        );
    }

    #[test]
    fn warm_analysis_sweep_preserves_reference_results() {
        let store = DocumentStore::new();
        store.ingest(
            uri("/svc.php"),
            "<?php\nnamespace App;\nclass Svc { public function run(): void {} }",
        );
        store.ingest(
            uri("/caller.php"),
            "<?php\nnamespace App;\nclass Caller {\n    private Svc $s;\n    public function go(): void { $this->s->run(); }\n}",
        );
        let sym = mir_analyzer::Name::method("App\\Svc", "run");
        let files: Vec<Arc<str>> = [uri("/svc.php"), uri("/caller.php")]
            .iter()
            .map(|u| Arc::from(u.as_str()))
            .collect();
        let cold = store.indexed_references(&sym, &files, false, None);
        assert_eq!(cold.len(), 1, "caller.php references Svc::run once");

        let cancel = store.begin_warm_sweep();
        store.warm_analysis_sweep(&[], &cancel);
        let warm = store.indexed_references(&sym, &files, false, None);
        assert_eq!(cold, warm, "sweep must not change reference results");

        // An edit after the sweep is still picked up (memos revalidate).
        store.ingest(
            uri("/caller.php"),
            "<?php\nnamespace App;\nclass Caller {\n    private Svc $s;\n    public function go(): void { $this->s->run(); $this->s->run(); }\n}",
        );
        let cancel = store.begin_warm_sweep();
        store.warm_analysis_sweep(&[], &cancel);
        let after_edit = store.indexed_references(&sym, &files, false, None);
        assert_eq!(after_edit.len(), 2, "re-sweep must see the new reference");
    }

    #[test]
    fn sweep_priority_puts_open_files_and_their_dependencies_first() {
        let store = DocumentStore::new();
        store.ingest(
            uri("/a.php"),
            "<?php\nnamespace App;\nclass A { public function go(B $b): void {} }",
        );
        store.ingest(uri("/b.php"), "<?php\nnamespace App;\nclass B {}");
        store.ingest(uri("/unrelated.php"), "<?php\nnamespace App;\nclass C {}");
        store.mark_index_ready();

        let front = store.sweep_priority_files(&[uri("/a.php")]);
        let front: Vec<&str> = front.iter().map(|f| f.as_ref()).collect();
        assert_eq!(
            front,
            vec![uri("/a.php").as_str(), uri("/b.php").as_str()],
            "open file first, then the files declaring its referenced classes"
        );
    }

    #[test]
    fn warm_analysis_sweep_stops_on_cancel() {
        let store = DocumentStore::new();
        for i in 0..40 {
            store.ingest(
                uri(&format!("/f{i}.php")),
                "<?php\nclass A { public function m(): int { return 1; } }",
            );
        }
        let cancel = store.begin_warm_sweep();
        cancel.cancel();
        // Pre-cancelled token: returns without analyzing (must not hang/panic).
        store.warm_analysis_sweep(&[], &cancel);
        // A newer sweep's token supersedes the old one.
        let old = store.begin_warm_sweep();
        let _new = store.begin_warm_sweep();
        assert!(old.is_cancelled());
    }

    #[test]
    fn all_indexes_includes_every_mirrored_file() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nfunction a() {}".to_string());
        store.ingest(uri("/b.php"), "<?php\nfunction b() {}");
        assert_eq!(store.all_indexes().len(), 2);
    }

    #[test]
    fn other_indexes_excludes_current_uri() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nfunction a() {}".to_string());
        open(&store, uri("/b.php"), "<?php\nfunction b() {}".to_string());
        assert_eq!(store.other_indexes(&uri("/a.php")).len(), 1);
    }

    #[test]
    fn other_docs_excludes_current_uri() {
        let store = DocumentStore::new();
        let ua = uri("/a.php");
        let ub = uri("/b.php");
        open(&store, ua.clone(), "<?php\nfunction a() {}".to_string());
        open(&store, ub.clone(), "<?php\nfunction b() {}".to_string());
        let open_urls = vec![ua.clone(), ub];
        assert_eq!(store.other_docs(&ua, &open_urls).len(), 1);
    }

    #[test]
    fn evict_token_cache_removes_entry() {
        let store = DocumentStore::new();
        let u = uri("/a.php");
        open(&store, u.clone(), "<?php".to_string());
        store.store_token_cache(&u, "id1".to_string(), Arc::new(vec![]));
        assert!(store.get_token_cache(&u, "id1").is_some());
        store.evict_token_cache(&u);
        assert!(store.get_token_cache(&u, "id1").is_none());
    }

    #[test]
    fn vendor_index_cache_evicted_on_remove() {
        let store = DocumentStore::new();
        let u = uri("/vendor/acme/lib.php");
        store.ingest(u.clone(), "<?php\nclass Lib {}");
        let idx = store.get_index_salsa(&u).unwrap();
        store.cache_vendor_index(u.clone(), idx.clone());
        assert!(store.get_vendor_index(&u).is_some());
        store.remove(&u);
        assert!(store.get_vendor_index(&u).is_none());
    }

    #[test]
    fn index_populates_file_index_with_symbols() {
        let store = DocumentStore::new();
        store.ingest(uri("/a.php"), "<?php\nfunction hello() {}");
        let idx = store.get_index_salsa(&uri("/a.php")).unwrap();
        assert_eq!(idx.functions.len(), 1);
        assert_eq!(&*idx.functions[0].name, "hello");
    }

    #[test]
    fn open_populates_file_index_with_symbols() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nclass Foo {}".to_string());
        let idx = store.get_index_salsa(&uri("/a.php")).unwrap();
        assert_eq!(idx.classes.len(), 1);
        assert_eq!(&*idx.classes[0].name, "Foo");
    }

    // ── Mirror invariants ────────────────────────────────────────────────
    //
    // Every mutation path that changes file text must keep the salsa layer
    // consistent. These tests walk a set-edit-reopen cycle and assert that
    // the salsa-derived `FileIndex` reflects the latest text at each step.

    fn names_of(idx: &FileIndex) -> Vec<String> {
        let mut out: Vec<String> = idx.classes.iter().map(|c| c.name.to_string()).collect();
        out.extend(idx.functions.iter().map(|f| f.name.to_string()));
        out.sort();
        out
    }

    fn salsa_index_names(store: &DocumentStore, url: &Url) -> Vec<String> {
        store
            .snapshot_query_file_index(url)
            .map(|idx| names_of(&idx))
            .unwrap_or_default()
    }

    #[test]
    fn mirror_tracks_repeated_edits() {
        let store = DocumentStore::new();
        let u = uri("/mirror.php");

        open(&store, u.clone(), "<?php\nclass A {}".to_string());
        assert_eq!(salsa_index_names(&store, &u), vec!["A".to_string()]);

        open(
            &store,
            u.clone(),
            "<?php\nclass A {}\nclass B {}".to_string(),
        );
        assert_eq!(
            salsa_index_names(&store, &u),
            vec!["A".to_string(), "B".to_string()]
        );

        open(&store, u.clone(), "<?php\nfunction greet() {}".to_string());
        assert_eq!(salsa_index_names(&store, &u), vec!["greet".to_string()]);
    }

    #[test]
    fn mirror_tracks_ingest_and_ingest_from_doc() {
        let store = DocumentStore::new();

        // Background `index(url, text)` path.
        let u1 = uri("/bg1.php");
        store.ingest(u1.clone(), "<?php\nclass Bg1 {}");
        assert_eq!(salsa_index_names(&store, &u1), vec!["Bg1".to_string()]);

        // `ingest_from_doc(url, &doc)` path (workspace-scan Phase 2).
        let u2 = uri("/bg2.php");
        let doc = crate::analysis::diagnostics::parse_document_no_diags(
            "<?php\nclass Bg2 {}\nfunction f() {}",
        );
        store.ingest_from_doc(u2.clone(), &doc);
        assert_eq!(
            salsa_index_names(&store, &u2),
            vec!["Bg2".to_string(), "f".to_string()]
        );
    }

    /// G3: confirms the `parsed_cache` actually hits — two consecutive
    /// `get_doc_salsa` calls on unchanged text return the same `Arc`
    /// (pointer equality), and an edit forces a miss that produces a
    /// different `Arc`.
    /// parsed_cache must stay bounded — inserting more than
    /// `PARSED_CACHE_CAP` unique URLs must not cause unbounded growth.
    /// Eviction is probabilistic, so we only assert the bound, not which
    /// Seeding a cached index for a URL that was never mirrored is a no-op
    /// (returns `false`) — avoids silently allocating SourceFiles outside
    /// `mirror_text`'s control.
    #[test]
    fn seed_cached_index_noops_for_unknown_uri() {
        let store = DocumentStore::new();
        let u = uri("/never_mirrored.php");
        let index = Arc::new(crate::index::file_index::FileIndex::default());
        assert!(!store.seed_cached_index(&u, index));
    }

    /// entries survive.
    #[test]
    fn parsed_cache_stays_bounded_under_many_inserts() {
        let store = DocumentStore::new();
        use crate::document::cache_registry::PARSED_CACHE_CAP;
        let overflow = PARSED_CACHE_CAP + 100;
        for i in 0..overflow {
            let u = uri(&format!("/cap/file{i}.php"));
            store.ingest(u.clone(), "<?php\nclass A {}");
            // Force a parsed_cache insert via get_doc_salsa.
            let _ = store.get_doc_salsa(&u);
        }
        assert!(
            store.caches.parsed_cache.len() <= PARSED_CACHE_CAP,
            "parsed_cache grew to {} entries (cap {})",
            store.caches.parsed_cache.len(),
            PARSED_CACHE_CAP
        );
    }

    /// The shed is recency-based: an entry touched right before overflow (an
    /// open file being edited) must survive a sweep of cold inserts, while
    /// untouched cold entries are the ones dropped.
    #[test]
    fn parsed_cache_shed_keeps_recently_used_entries() {
        let store = DocumentStore::new();
        use crate::document::cache_registry::PARSED_CACHE_CAP;

        let hot = uri("/cap/hot.php");
        store.ingest(hot.clone(), "<?php\nclass Hot {}");
        let _ = store.get_doc_salsa(&hot);

        // Fill to just below the cap with cold entries, then re-touch the hot
        // file so it is the most recently used entry at shed time.
        for i in 0..(PARSED_CACHE_CAP - 2) {
            let u = uri(&format!("/cap/cold{i}.php"));
            store.ingest(u.clone(), "<?php\nclass A {}");
            let _ = store.get_doc_salsa(&u);
        }
        let _ = store.get_doc_salsa(&hot);

        // Overflow: this insert triggers the shed of the LRU half.
        let trigger = uri("/cap/trigger.php");
        store.ingest(trigger.clone(), "<?php\nclass T {}");
        let _ = store.get_doc_salsa(&trigger);

        assert!(
            store.caches.parsed_cache.contains_key(&hot),
            "recently-touched entry must survive the recency shed"
        );
        assert!(
            store.caches.parsed_cache.len() <= PARSED_CACHE_CAP,
            "cache must stay bounded after the shed"
        );
    }

    /// `analysis_cache` must stay bounded when many distinct files are
    /// analyzed across a session (multi-hour usage previously grew it
    /// without limit).
    #[test]
    fn analysis_cache_stays_bounded_under_many_files() {
        use crate::document::cache_registry::ANALYSIS_CACHE_CAP;
        let store = DocumentStore::new();
        let overflow = ANALYSIS_CACHE_CAP + 16;
        for i in 0..overflow {
            let u = uri(&format!("/acap/file{i}.php"));
            store.ingest(u.clone(), "<?php\nfunction f() { return 1; }");
            let _ = store.cached_analysis(&u);
        }
        assert!(
            store.caches.analysis_cache.len() <= ANALYSIS_CACHE_CAP,
            "analysis_cache grew to {} entries (cap {})",
            store.caches.analysis_cache.len(),
            ANALYSIS_CACHE_CAP
        );
    }

    #[test]
    fn get_doc_salsa_cache_hits_across_calls() {
        let store = DocumentStore::new();
        let u = uri("/g3_cache.php");
        open(&store, u.clone(), "<?php\nclass G3 {}".to_string());

        let a = store.get_doc_salsa(&u).unwrap();
        let b = store.get_doc_salsa(&u).unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "parsed_cache hit should yield the same Arc across calls"
        );

        open(&store, u.clone(), "<?php\nclass G3b {}".to_string());
        let c = store.get_doc_salsa(&u).unwrap();
        assert!(
            !Arc::ptr_eq(&a, &c),
            "edit should invalidate the parsed_cache entry"
        );
    }

    #[test]
    fn get_doc_salsa_returns_some_for_mirrored_files() {
        // Phase E4: `get_doc_salsa` no longer gates on open-state. The
        // open/closed distinction now lives on `Backend::get_doc`.
        let store = DocumentStore::new();
        let u = uri("/e4_doc.php");
        store.ingest(u.clone(), "<?php\nclass P {}");
        assert!(store.get_doc_salsa(&u).is_some());
    }

    #[test]
    fn get_doc_snapshot_or_stale_matches_salsa_on_settled_buffer() {
        let store = DocumentStore::new();
        let u = uri("/stale_settled.php");
        open(&store, u.clone(), "<?php\nclass S {}".to_string());
        let fresh = store.get_doc_salsa(&u).unwrap();
        let stale = store.get_doc_snapshot_or_stale(&u).unwrap();
        assert!(
            Arc::ptr_eq(&fresh, &stale),
            "on a settled buffer the stale-tolerant accessor must serve the cached parse"
        );
    }

    /// WS1: the cursor-triggered highlight accessor must stay responsive under a
    /// sustained write stream — never spin (the deadline join hangs if it does)
    /// and always yield a fresh or last-good parse for an open file.
    #[test]
    fn get_doc_snapshot_or_stale_stays_responsive_under_writes() {
        use std::sync::Arc;
        use std::thread;
        use std::time::{Duration, Instant};

        let store = Arc::new(DocumentStore::new());
        let urls: Vec<Url> = (0..8).map(|i| uri(&format!("/hl{i}.php"))).collect();
        for (i, u) in urls.iter().enumerate() {
            open(&store, u.clone(), format!("<?php\nclass H{i} {{}}"));
            // Warm the parse cache so the stale fallback always has a last-good doc.
            assert!(store.get_doc_salsa(u).is_some());
        }

        let deadline = Instant::now() + Duration::from_millis(400);
        let mut handles = Vec::new();

        {
            let store = Arc::clone(&store);
            let urls = urls.clone();
            handles.push(thread::spawn(move || {
                let mut rev = 0u32;
                while Instant::now() < deadline {
                    for u in &urls {
                        store.mirror_text(u, &format!("<?php\nclass H {{}}\n// rev {rev}"));
                    }
                    rev += 1;
                }
            }));
        }

        for _ in 0..4 {
            let store = Arc::clone(&store);
            let urls = urls.clone();
            handles.push(thread::spawn(move || {
                while Instant::now() < deadline {
                    for u in &urls {
                        assert!(
                            store.get_doc_snapshot_or_stale(u).is_some(),
                            "open file must resolve to a fresh or stale parse, never None"
                        );
                    }
                }
            }));
        }

        for h in handles {
            h.join()
                .expect("stale accessor must not panic or spin under concurrent writes");
        }
    }

    /// WS1 A/B measurement (not a pass/fail guard): compares the old unbounded
    /// `get_doc_salsa` against the new bounded `get_doc_snapshot_or_stale` under
    /// an identical continuous write stream. Run explicitly:
    /// `cargo test --lib ws1_ab_latency -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing measurement; run with --ignored --nocapture"]
    fn ws1_ab_latency_under_write_pressure() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;
        use std::time::{Duration, Instant};

        // A large file: each parse is long enough that a concurrent write can
        // cancel it mid-flight — the condition under which the unbounded retry
        // actually spins (a tiny file parses faster than the writer's gap).
        fn big_source(rev: u32) -> String {
            let mut s = String::from("<?php\nclass Big {\n");
            for i in 0..3000 {
                s.push_str(&format!(
                    "    public function m{i}(int $a, string $b): int {{ return $a + {i}; }}\n"
                ));
            }
            s.push_str(&format!("}}\n// rev {rev}\n"));
            s
        }

        fn measure(
            store: &Arc<DocumentStore>,
            url: &Url,
            writers: usize,
            label: &str,
            call: impl Fn(&DocumentStore, &Url) -> bool,
        ) {
            let stop = Arc::new(AtomicBool::new(false));
            let mut writer_handles = Vec::new();
            for w in 0..writers {
                let store = Arc::clone(store);
                let url = url.clone();
                let stop = Arc::clone(&stop);
                writer_handles.push(thread::spawn(move || {
                    let mut rev = (w as u32) * 1_000_000;
                    while !stop.load(Ordering::Relaxed) {
                        store.mirror_text(&url, &big_source(rev));
                        rev = rev.wrapping_add(1);
                    }
                }));
            }
            thread::sleep(Duration::from_millis(30)); // let the writers get hot

            let n = 100usize;
            let mut max = Duration::ZERO;
            let mut total = Duration::ZERO;
            for _ in 0..n {
                let t = Instant::now();
                let ok = call(store, url);
                let dt = t.elapsed();
                assert!(ok, "{label}: accessor returned None");
                max = max.max(dt);
                total += dt;
            }
            stop.store(true, Ordering::Relaxed);
            for h in writer_handles {
                h.join().unwrap();
            }
            println!(
                "{label}: {n} calls, {writers} writers — max {:>10.3?}, mean {:>10.3?}",
                max,
                total / n as u32
            );
        }

        let store = Arc::new(DocumentStore::new());
        let url = uri("/perf_big.php");
        open(&store, url.clone(), big_source(0));
        assert!(store.get_doc_salsa(&url).is_some()); // warm the cache

        for writers in [2usize, 4] {
            measure(
                &store,
                &url,
                writers,
                "NEW get_doc_snapshot_or_stale",
                |s, u| s.get_doc_snapshot_or_stale(u).is_some(),
            );
            measure(
                &store,
                &url,
                writers,
                "OLD get_doc_salsa (unbounded)  ",
                |s, u| s.get_doc_salsa(u).is_some(),
            );
        }
    }

    #[test]
    fn get_salsa_accessors_return_none_for_unknown_uri() {
        let store = DocumentStore::new();
        let u = uri("/never-seen.php");
        assert!(store.get_doc_salsa(&u).is_none());
        assert!(store.get_index_salsa(&u).is_none());
    }

    /// Phase E1: concurrent readers and writers must not deadlock, panic, or
    /// return stale data. Writers briefly bump inputs while readers are
    /// running on cloned snapshots; any `salsa::Cancelled` raised on the
    /// reader side must be caught and retried by `snapshot_query`.
    ///
    /// The salsa surface (`get_doc_salsa`, `get_index_salsa`) is protected by
    /// `snapshot_query`'s last-resort host-lock fallback.
    #[test]
    fn concurrent_reads_and_writes_do_not_panic() {
        use std::sync::Arc;
        use std::thread;
        use std::time::{Duration, Instant};

        let store = Arc::new(DocumentStore::new());
        let urls: Vec<Url> = (0..8).map(|i| uri(&format!("/f{i}.php"))).collect();
        for (i, u) in urls.iter().enumerate() {
            open(&store, u.clone(), format!("<?php\nclass C{i} {{}}"));
        }

        let deadline = Instant::now() + Duration::from_millis(400);
        let mut handles = Vec::new();

        // Writer thread: keep bumping every file's text.
        {
            let store = Arc::clone(&store);
            let urls = urls.clone();
            handles.push(thread::spawn(move || {
                let mut rev = 0u32;
                while Instant::now() < deadline {
                    for u in &urls {
                        let text = format!("<?php\nclass C{{}}\n// rev {rev}");
                        store.mirror_text(u, &text);
                    }
                    rev += 1;
                }
            }));
        }

        // Reader threads: hammer the salsa accessors.
        for _ in 0..4 {
            let store = Arc::clone(&store);
            let urls = urls.clone();
            handles.push(thread::spawn(move || {
                while Instant::now() < deadline {
                    for u in &urls {
                        let _ = store.get_doc_salsa(u);
                        let _ = store.get_index_salsa(u);
                    }
                    // Post mir 0.22: codebase + refs live in the session,
                    // not salsa. Concurrent-read smoke is limited to the
                    // remaining salsa surface (parsed_doc, file_index).
                }
            }));
        }

        for h in handles {
            h.join().expect("no panic under concurrent read/write");
        }
    }

    /// PSR-4 lazy-loading: `get_semantic_issues_salsa` must not emit
    /// `UndefinedClass` for a class that is PSR-4-resolvable on disk, even
    /// when the dependency file is not yet in `source_files`.
    #[test]
    fn psr4_lazy_load_suppresses_undefined_class() {
        let tmp = tempfile::tempdir().unwrap();

        // Write Entity.php to disk (not mirrored into the store).
        std::fs::create_dir_all(tmp.path().join("src/Model")).unwrap();
        std::fs::write(
            tmp.path().join("src/Model/Entity.php"),
            "<?php\nnamespace App\\Model;\nclass Entity {}\n",
        )
        .unwrap();

        // Write composer.json so Psr4Map::load can build the map.
        std::fs::write(
            tmp.path().join("composer.json"),
            r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#,
        )
        .unwrap();

        let store = DocumentStore::new();

        // Inject a PSR-4 map pointing at the tmp dir.
        store
            .psr4
            .store(Arc::new(crate::lang::autoload::Psr4Map::load(tmp.path())));

        // Mirror the consuming file (Entity not yet in source_files).
        // Uses Entity as a parameter type hint — the analyzer resolves these
        // through use statements, so this exercises the full PSR-4 lazy-load path.
        let handler_url = Url::from_file_path(tmp.path().join("src/Service/Handler.php")).unwrap();
        store.mirror_text(
            &handler_url,
            "<?php\nnamespace App\\Service;\nuse App\\Model\\Entity;\nfunction handle(Entity $e): Entity { return $e; }\n",
        );

        let issues = store.get_semantic_issues_salsa(&handler_url).unwrap();
        let undef: Vec<_> = issues
            .iter()
            .filter(|i| matches!(i.kind, mir_issues::IssueKind::UndefinedClass { .. }))
            .collect();
        assert!(
            undef.is_empty(),
            "PSR-4 lazy-loading must prevent UndefinedClass for App\\Model\\Entity; got: {undef:?}"
        );
    }

    /// Issue #191 regression: workspace-wide scans (find-references, rename,
    /// call-hierarchy) must not re-parse closed/indexed files on repeated
    /// invocations. Once a file's `ParsedDoc` has been produced, subsequent
    /// `all_docs_for_scan()` calls must hit the cache and return the same
    /// `Arc<ParsedDoc>` (pointer equality), proving no re-parse occurred.
    ///
    /// The cache layers protecting this are:
    ///   1. `parsed_cache` (cap [`PARSED_CACHE_CAP`]) — read-through, validated
    ///      via `Arc::ptr_eq` on the text Arc.
    ///   2. salsa `parsed_doc` memo (`lru = 2048`) — second line of defense
    ///      when `parsed_cache` evicts.
    ///
    /// Together they keep every workspace-scan op O(N) memo lookups, never
    /// O(N) parses, for any workspace whose file count fits the cap.
    #[test]
    fn all_docs_for_scan_does_not_reparse_indexed_files() {
        let store = DocumentStore::new();
        const N: usize = 50;
        for i in 0..N {
            let u = uri(&format!("/scan/file{i}.php"));
            store.ingest(u, &format!("<?php\nclass C{i} {{}}\nfunction f{i}() {{}}"));
        }

        let first: Vec<_> = store.all_docs_for_scan();
        let second: Vec<_> = store.all_docs_for_scan();
        assert_eq!(first.len(), N);
        assert_eq!(second.len(), N);

        let by_url_first: std::collections::HashMap<Url, Arc<ParsedDoc>> =
            first.into_iter().collect();
        for (u, doc2) in second {
            let doc1 = by_url_first
                .get(&u)
                .expect("second scan returned a URL the first didn't");
            assert!(
                Arc::ptr_eq(doc1, &doc2),
                "{u} re-parsed across all_docs_for_scan calls — \
                 cache (parsed_cache + salsa parsed_doc memo) failed to hit"
            );
        }

        // Editing one file's text must invalidate just that file's entry,
        // not the rest. This locks in self-eviction via Arc::ptr_eq on text.
        let edited_url = uri("/scan/file0.php");
        let pre_edit = store.get_doc_salsa(&edited_url).unwrap();
        store.ingest(edited_url.clone(), "<?php\nclass C0Edited {}");
        let post_edit = store.get_doc_salsa(&edited_url).unwrap();
        assert!(
            !Arc::ptr_eq(&pre_edit, &post_edit),
            "edited file must produce a fresh ParsedDoc"
        );
        for i in 1..N {
            let u = uri(&format!("/scan/file{i}.php"));
            let original = by_url_first.get(&u).unwrap();
            let after = store.get_doc_salsa(&u).unwrap();
            assert!(
                Arc::ptr_eq(original, &after),
                "{u} should not have re-parsed because of an unrelated edit"
            );
        }
    }

    /// Incremental analysis cache: a body-only edit to file A (no declaration
    /// changes) must not bump `decl_version`, so file B's cached analysis
    /// survives. A declaration edit MUST bump the version so B's entry goes
    /// stale.
    #[test]
    fn body_only_edit_does_not_invalidate_sibling_analysis_cache() {
        let store = DocumentStore::new();
        let ua = uri("/ic_a.php");
        let ub = uri("/ic_b.php");

        // Analyze both files to establish their fingerprints.
        open(
            &store,
            ua.clone(),
            "<?php\nfunction a() { return 1; }".to_string(),
        );
        open(
            &store,
            ub.clone(),
            "<?php\nfunction b() { return 2; }".to_string(),
        );
        let _ = store.cached_analysis(&ua).unwrap();
        let analysis_b_first = store.cached_analysis(&ub).unwrap();
        let ver_after_warm = store.caches.decl_version();

        // Body-only edit to A: same function name, different body → FileIndex unchanged.
        store.mirror_text(&ua, "<?php\nfunction a() { return 999; }");
        let _ = store.cached_analysis(&ua);
        let ver_after_body_edit = store.caches.decl_version();
        assert_eq!(
            ver_after_warm, ver_after_body_edit,
            "body-only edit must not bump decl_version"
        );

        // B's cached entry should still be valid (ptr-eq source AND same version).
        let analysis_b_second = store.cached_analysis_if_fresh(&ub);
        assert!(
            analysis_b_second.is_some(),
            "B's analysis should hit cache after body-only edit to A"
        );
        assert!(
            Arc::ptr_eq(&analysis_b_first, &analysis_b_second.unwrap()),
            "B's analysis should be the identical Arc (no re-analysis)"
        );

        // Declaration edit to A: rename the function → FileIndex changes.
        store.mirror_text(&ua, "<?php\nfunction a_renamed() { return 999; }");
        let _ = store.cached_analysis(&ua);
        let ver_after_decl_edit = store.caches.decl_version();
        assert!(
            ver_after_decl_edit > ver_after_body_edit,
            "declaration edit must bump decl_version (was {ver_after_body_edit}, now {ver_after_decl_edit})"
        );

        // B's entry is now stale — cached_analysis_if_fresh must return None.
        let analysis_b_stale = store.cached_analysis_if_fresh(&ub);
        assert!(
            analysis_b_stale.is_none(),
            "B's analysis should be stale after A's declaration changed"
        );
    }

    /// snapshot_query must complete without panic when a concurrent writer
    /// races the snapshot. The single-retry-then-lock logic should handle this
    /// correctly: the lock-held fallback guarantees progress.
    #[test]
    fn snapshot_query_survives_concurrent_writes() {
        use std::sync::Arc;
        use std::thread;
        use std::time::{Duration, Instant};

        let store = Arc::new(DocumentStore::new());
        let u = uri("/sq_test.php");
        open(
            &store,
            u.clone(),
            "<?php\nfunction f(): int { return 1; }".to_string(),
        );

        let deadline = Instant::now() + Duration::from_millis(200);
        let mut handles = Vec::new();

        // Writer: keep bumping the file text to trigger salsa::Cancelled.
        {
            let store = Arc::clone(&store);
            let u = u.clone();
            handles.push(thread::spawn(move || {
                let mut rev = 0u32;
                while Instant::now() < deadline {
                    store.mirror_text(&u, &format!("<?php\nfunction f(): int {{ return {rev}; }}"));
                    rev += 1;
                }
            }));
        }

        // Reader: hammer snapshot_query via get_doc_salsa.
        for _ in 0..4 {
            let store = Arc::clone(&store);
            let u = u.clone();
            handles.push(thread::spawn(move || {
                while Instant::now() < deadline {
                    let _ = store.get_doc_salsa(&u);
                    let _ = store.get_index_salsa(&u);
                }
            }));
        }

        for h in handles {
            h.join()
                .expect("no panic in snapshot_query under concurrent writes");
        }
    }

    /// When a sibling file's declaration changes (bumping decl_version), the
    /// owned_program_cache entry for the unchanged file B should be reused
    /// rather than deep-cloned again. We verify this via Arc pointer equality.
    #[test]
    fn owned_program_cache_reused_after_sibling_declaration_change() {
        let store = DocumentStore::new();
        let ua = uri("/prog_a.php");
        let ub = uri("/prog_b.php");

        open(
            &store,
            ua.clone(),
            "<?php\nfunction alpha(): void {}".to_string(),
        );
        open(
            &store,
            ub.clone(),
            "<?php\nfunction beta(): void {}".to_string(),
        );

        // Warm both analysis caches and populate owned_program_cache for both files.
        let _ = store.cached_analysis(&ua);
        let _ = store.cached_analysis(&ub);

        // Capture B's owned_program Arc before the sibling edit.
        let prog_b_first = store
            .caches
            .owned_program_cache
            .get(&ub)
            .map(|e| Arc::clone(&e.1))
            .expect("B's owned_program should be cached after first analysis");

        // Declaration change to A: bumps decl_version, invalidating all cached_analysis entries.
        store.mirror_text(&ua, "<?php\nfunction alpha_renamed(): void {}");
        // Re-analyze A to trigger the decl_version bump.
        let _ = store.cached_analysis(&ua);

        // Now re-analyze B. Its source is unchanged, so owned_program_cache must hit.
        let _ = store.cached_analysis(&ub);

        let prog_b_second = store
            .caches
            .owned_program_cache
            .get(&ub)
            .map(|e| Arc::clone(&e.1))
            .expect("B's owned_program should still be cached after sibling edit");

        assert!(
            Arc::ptr_eq(&prog_b_first, &prog_b_second),
            "B's owned_program Arc should be identical (cache hit) after sibling declaration change"
        );
    }

    /// write_rev() increments on each real text write (identical text is a no-op).
    #[test]
    fn write_rev_increments_on_write() {
        let store = DocumentStore::new();
        let u = uri("/rev_test.php");
        let before = store.write_rev();
        store.mirror_text(&u, "<?php echo 1;");
        let after_first = store.write_rev();
        assert!(after_first > before, "first write must bump the revision");
        // Identical text must NOT bump the revision (fast path).
        store.mirror_text(&u, "<?php echo 1;");
        assert_eq!(
            store.write_rev(),
            after_first,
            "identical text must not bump the revision"
        );
        // Different text must bump again.
        store.mirror_text(&u, "<?php echo 2;");
        assert!(
            store.write_rev() > after_first,
            "changed text must bump the revision"
        );
    }

    /// begin_reanalyze() cancels the previously-issued sweep token so a newer
    /// edit preempts an in-flight dependent walk, and hands back a fresh token.
    #[test]
    fn begin_reanalyze_cancels_previous_token() {
        let store = DocumentStore::new();
        let first = store.begin_reanalyze();
        assert!(!first.is_cancelled(), "a fresh token starts un-cancelled");
        let second = store.begin_reanalyze();
        assert!(
            first.is_cancelled(),
            "issuing a new token must cancel the previous in-flight sweep"
        );
        assert!(
            !second.is_cancelled(),
            "the newly-issued token must itself be un-cancelled"
        );
    }

    /// A session built before the cache dir is known (an editor's
    /// restored-buffer `didOpen` racing `initialized`) must be rebuilt with
    /// the cache attached when the dir arrives — not stay pinned
    /// in-memory-only for the server's lifetime.
    #[test]
    fn set_session_cache_dir_rebuilds_pinned_session() {
        let store = DocumentStore::new();
        let early = store.analysis_session(mir_analyzer::PhpVersion::LATEST);
        let dir = tempfile::tempdir().unwrap();
        store.set_session_cache_dir(dir.path().to_path_buf());
        let rebuilt = store.analysis_session(mir_analyzer::PhpVersion::LATEST);
        assert!(
            !Arc::ptr_eq(&early, &rebuilt),
            "the cache-less early session must be dropped and rebuilt"
        );
        assert!(
            dir.path().join("stubs").exists(),
            "the rebuilt session must have opened the on-disk stub cache"
        );
    }

    /// write_rev() increments when a file is removed.
    #[test]
    fn write_rev_increments_on_remove() {
        let store = DocumentStore::new();
        let u = uri("/rev_remove.php");
        store.mirror_text(&u, "<?php class A {}");
        let before = store.write_rev();
        store.remove(&u);
        assert!(store.write_rev() > before, "remove must bump the revision");
    }
}
