use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use arc_swap::{ArcSwap, ArcSwapOption};

use dashmap::{DashMap, DashSet};
use salsa::Setter;
use tower_lsp_server::ls_types::{SemanticToken, Uri};

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
    /// `Uri -> LspWsFile` lookup on the shared mir db. Each `LspWsFile` pairs
    /// mir's `SourceFile` (the shared text input) with the optional warm-start
    /// `cached_index`. Created/updated through the `AnalysisSession`'s
    /// `with_db_mut` under the db write lock; reads run on cheap snapshot clones.
    lsp_ws_files: DashMap<Uri, LspWsFile>,
    /// URIs that have been removed. Re-opening a deleted URI un-deletes it here
    /// and reuses the existing `LspWsFile` handle.
    deleted_uris: DashSet<Uri>,
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
    /// Diagnostic counter: incremented once per call to
    /// `resolve_reachability_queries`, i.e. once per actual workspace scan
    /// pass — regardless of how many queries are batched into that pass.
    /// Lets tests assert that batching N symbols' reachability narrowing
    /// (e.g. all of one file's code lenses) pays one scan, not N.
    reachability_scan_passes: AtomicU64,
    /// Memoized [`Self::resolve_reachability_queries`] results, keyed on the
    /// query plus [`Self::write_revision`]. Without this, a warm repeat
    /// still pays the O(workspace files) outer loop every call even when
    /// mir's `ClassMentionIndex` avoids the expensive text scan inside it —
    /// cheap per-file work times tens of thousands of files is still
    /// measurable (confirmed via `references_all_kinds` bench: ~1.5-2ms on
    /// a 36k-file workspace, for exactly the query shapes that reach this
    /// function — `class`/`function`/`global constant`/a narrowable
    /// `method` — while shapes answered by a direct index lookup, never
    /// reaching this function at all, stayed sub-0.5ms). Bounded by total
    /// cached FILE entries (`reachability_result_cache_files`), not entry
    /// count, same discipline as mir's own per-file mention cache.
    reachability_result_cache: ReachabilityResultCache,
    /// Sum of `.len()` across every `reachability_result_cache` value.
    reachability_result_cache_files: AtomicU64,
    /// Per-URI lock so concurrent callers computing the same uncached file's
    /// analysis (`cached_analysis_cancellable`) serialize onto one
    /// computation instead of each redoing the full mir pass — e.g. `did_open`'s
    /// diagnostics trigger racing a fast-following hover on a large,
    /// just-opened file. Entries are never removed: bounded by the number of
    /// distinct files ever analyzed, the same order of magnitude as
    /// `lsp_ws_files`.
    analysis_inflight: DashMap<Uri, Arc<Mutex<()>>>,
    /// Diagnostic counter: incremented once per real `FileAnalyzer::analyze`
    /// run in `cached_analysis_cancellable` (never on a cache hit). Lets
    /// tests assert that concurrent callers racing for the same uncached
    /// file's analysis (see `analysis_inflight`) compute it once, not once
    /// per caller.
    analysis_compute_count: AtomicU64,
    /// `LspWorkspace` salsa input on the shared mir db: the project-file scoping
    /// set aggregated by `workspace_index`. Created lazily on first sync (the db
    /// is owned by the lazily-built `AnalysisSession`).
    lsp_workspace: Mutex<Option<LspWorkspace>>,
    /// Shared PSR-4 namespace-to-path map. Shared with `Backend` via `Arc`
    /// so updates from `initialized` (when composer.json is loaded) are
    /// visible here without any additional wiring. `ArcSwap` makes reads
    /// lock-free — a poisoned guard can no longer crash a request handler.
    psr4: Arc<ArcSwap<Psr4Map>>,
    /// `(target PHP version, cached AnalysisSession built for that version)`.
    /// One lock for both — `workspace_php_version()` used to read a separate
    /// `Mutex<PhpVersion>`, so fetching the version and then building/fetching
    /// the session used to take two locks per call; `current_analysis_session()`
    /// takes one. `None` only before the first build, or right after
    /// `set_php_version` invalidates it.
    /// mir-analyzer's `AnalysisSession` owns the workspace MirDb, runs Pass-2
    /// analysis, and lazy-loads dependencies via PSR-4.
    analysis_session: Mutex<(
        mir_analyzer::PhpVersion,
        Option<Arc<mir_analyzer::AnalysisSession>>,
    )>,
    /// Cache directory shared with the workspace file-index cache. When set,
    /// new `AnalysisSession`s are built with `with_cache_dir` so that stub
    /// parsing results survive server restarts.
    session_cache_dir: OnceLock<std::path::PathBuf>,
    /// User-supplied stub directories (`initializationOptions.stubDirs` /
    /// `.php-lsp.json`). When set, new `AnalysisSession`s are built with
    /// `with_user_stubs` so their `.php` files are registered as the
    /// highest-precedence symbol source alongside the bundled built-ins.
    user_stub_dirs: OnceLock<Vec<std::path::PathBuf>>,
    /// URIs of autoload.files entries from composer.json. These define global
    /// helper functions (e.g. tap, class_uses_recursive in Laravel) that are
    /// not discoverable by namespace walk. Pre-ingested into the AnalysisSession
    /// before each file analysis so mir doesn't emit false UndefinedFunction.
    autoload_uris: std::sync::RwLock<Vec<Uri>>,
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
    /// Calls to [`Self::warm_start_indexes`] that actually replayed at least
    /// one file (not the empty-workspace no-op). Observability only, surfaced
    /// via `$/php-lsp/debugStats` so tests can await a runtime-added folder's
    /// warm-start replay instead of guessing a fixed delay.
    warm_start_replays_completed: AtomicU64,
    /// Files `warm_start_indexes` handed to a background reanalysis because
    /// mir's `warm_start_files` flagged their replayed reference postings as
    /// untrusted (a disk-cache replay of an unresolved-name commit — see
    /// mir's 0.67.0 changelog entry). `Arc`-wrapped so the detached
    /// reanalysis thread spawned by `warm_start_indexes` can bump it without
    /// borrowing `self`. Observability only, surfaced via
    /// `$/php-lsp/debugStats` so a protocol test can assert the reanalysis
    /// happened in the background, without ever issuing a query.
    warm_start_untrusted_reanalyzed: Arc<AtomicU64>,
    /// Throttled/idle-priority vendor warm-analysis sweeps run to completion
    /// (only meaningful when `warmVendorAnalysis: true` — see `LspConfig`).
    /// Always 0 until that sweep is implemented (ROADMAP 0c step 2,
    /// `~/.claude/plans/crispy-noodling-key.md`). Observability only,
    /// surfaced via `$/php-lsp/debugStats` so tests can await vendor warmth
    /// the same way `warm_sweeps_completed` lets them await the main sweep.
    vendor_warm_sweeps_completed: AtomicU64,
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
            reachability_scan_passes: AtomicU64::new(0),
            reachability_result_cache: DashMap::new(),
            reachability_result_cache_files: AtomicU64::new(0),
            analysis_inflight: DashMap::new(),
            analysis_compute_count: AtomicU64::new(0),
            lsp_workspace: Mutex::new(None),
            psr4: Arc::new(ArcSwap::from_pointee(Psr4Map::empty())),
            analysis_session: Mutex::new((mir_analyzer::PhpVersion::LATEST, None)),
            session_cache_dir: OnceLock::new(),
            user_stub_dirs: OnceLock::new(),
            autoload_uris: std::sync::RwLock::new(Vec::new()),
            index_ready: AtomicBool::new(false),
            write_revision: AtomicU64::new(0),
            reanalyze_cancel: Mutex::new(mir_analyzer::IndexCancel::new()),
            warm_sweep_cancel: Mutex::new(mir_analyzer::IndexCancel::new()),
            warm_sweeps_completed: AtomicU64::new(0),
            warm_start_replays_completed: AtomicU64::new(0),
            warm_start_untrusted_reanalyzed: Arc::new(AtomicU64::new(0)),
            vendor_warm_sweeps_completed: AtomicU64::new(0),
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
    pub fn warm_analysis_sweep(&self, priority: &[Uri], cancel: &mir_analyzer::IndexCancel) {
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

    /// Build the sweep's ordered file list: `priority` files and the files
    /// declaring the classes they reference (see [`Self::sweep_priority_files`])
    /// first, then every other tracked file — except vendor files, which the
    /// ambient sweep skips regardless of `indexVendor`. A vendor file that
    /// ends up in `priority` (opened directly, or referenced by an open
    /// project file) is unaffected: the exclusion only applies to the
    /// untargeted bulk-sweep tail, not the front of the queue.
    fn sweep_candidate_files(&self, priority: &[Uri]) -> Vec<Arc<str>> {
        let front: Vec<Arc<str>> = self.sweep_priority_files(priority);
        let front_set: HashSet<&str> = front.iter().map(|f| f.as_ref()).collect();
        front
            .iter()
            .cloned()
            .chain(
                self.lsp_ws_files
                    .iter()
                    .filter(|e| !self.deleted_uris.contains(e.key()))
                    .filter(|e| !is_vendor_uri(e.key()))
                    .map(|e| Arc::<str>::from(e.key().as_str()))
                    .filter(|f| !front_set.contains(f.as_ref())),
            )
            .collect()
    }

    fn warm_analysis_sweep_inner(&self, priority: &[Uri], cancel: &mir_analyzer::IndexCancel) {
        const CHUNK: usize = 32;
        let files = self.sweep_candidate_files(priority);
        let session = self.current_analysis_session();
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
        self.current_analysis_session().flush_analysis_cache();
    }

    /// Warm sweeps that ran to completion. See `$/php-lsp/debugStats`.
    pub fn warm_sweeps_completed(&self) -> u64 {
        self.warm_sweeps_completed.load(Ordering::Relaxed)
    }

    pub fn warm_start_replays_completed(&self) -> u64 {
        self.warm_start_replays_completed.load(Ordering::Relaxed)
    }

    /// See the `warm_start_untrusted_reanalyzed` field's docs.
    pub fn warm_start_untrusted_reanalyzed(&self) -> u64 {
        self.warm_start_untrusted_reanalyzed.load(Ordering::Relaxed)
    }

    /// See the `vendor_warm_sweeps_completed` field's docs.
    pub fn vendor_warm_sweeps_completed(&self) -> u64 {
        self.vendor_warm_sweeps_completed.load(Ordering::Relaxed)
    }

    /// The sweep's front of the queue: `priority` files themselves plus the
    /// files declaring the classes they reference (type hints, `use` imports,
    /// `new`, `extends`, …) — the set a request against an open file most
    /// likely touches. Resolution goes through the memoized workspace index,
    /// matching by FQN with a short-name fallback.
    fn sweep_priority_files(&self, priority: &[Uri]) -> Vec<Arc<str>> {
        if priority.is_empty() {
            return Vec::new();
        }
        let ws = self.get_workspace_index_salsa();
        let mut out: Vec<Arc<str>> = Vec::new();
        let mut seen: HashSet<Arc<str>> = HashSet::new();
        let push = |uri: &Uri, out: &mut Vec<Arc<str>>, seen: &mut HashSet<Arc<str>>| {
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
    /// with the cache attached.
    ///
    /// Returns `true` when a session was dropped — the caller must then
    /// re-mirror every currently open buffer (see `drop_session_scoped_state`):
    /// the workspace scan won't do it, since it explicitly skips files that
    /// are already open.
    pub fn set_session_cache_dir(&self, dir: std::path::PathBuf) -> bool {
        if self.session_cache_dir.set(dir).is_err() {
            return false;
        }
        let dropped = self.analysis_session.lock().unwrap().1.take().is_some();
        if dropped {
            self.drop_session_scoped_state();
        }
        dropped
    }

    /// Set the user-supplied stub directories (`initializationOptions.stubDirs`).
    /// Subsequent calls are silently ignored (`OnceLock` semantics).
    ///
    /// Same early-session race as [`Self::set_session_cache_dir`]: if a
    /// session was already built without these directories, drop it so the
    /// next `analysis_session()` rebuilds with `with_user_stubs` attached.
    ///
    /// Returns `true` when a session was dropped — same re-mirroring
    /// obligation as `set_session_cache_dir`.
    pub fn set_user_stub_dirs(&self, dirs: Vec<std::path::PathBuf>) -> bool {
        if self.user_stub_dirs.set(dirs).is_err() {
            return false;
        }
        let dropped = self.analysis_session.lock().unwrap().1.take().is_some();
        if dropped {
            self.drop_session_scoped_state();
        }
        dropped
    }

    /// Register URIs discovered from composer.json `autoload.files` entries.
    /// These PHP files define global helper functions (e.g. `tap()` in Laravel)
    /// that are not class-resolvable via PSR-4. Clears `analysis_cache` so the
    /// next per-file analysis pre-ingests them into the AnalysisSession before
    /// running mir's FileAnalyzer.
    pub fn set_autoload_uris(&self, uris: Vec<Uri>) {
        *self.autoload_uris.write().unwrap() = uris;
        self.caches.evict_analysis_all();
    }

    /// Get or build the `AnalysisSession` for the given PHP version. Rebuilds
    /// when the version changes (e.g. user flipped config). The session owns
    /// the shared salsa db and AnalysisCache; lazy-loads vendor files via the
    /// shared PSR-4 map. Built-in stubs are *not* pre-loaded: mir's
    /// `prepare_ast_for_analysis` ingests the stubs each analyzed file
    /// references, and [`crate::types::stub_members`] faults in single stub
    /// files for builtin member/hover lookups. User stub directories (see
    /// [`Self::set_user_stub_dirs`]) are configured on the builder here; mir's
    /// `ingest_file` — called for every mirrored document — is what actually
    /// registers them (and the built-in stubs) as `SourceFile` inputs.
    pub fn analysis_session(
        &self,
        php_version: mir_analyzer::PhpVersion,
    ) -> Arc<mir_analyzer::AnalysisSession> {
        let mut guard = self.analysis_session.lock().unwrap();
        if guard.0 == php_version
            && let Some(session) = guard.1.as_ref()
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
        // Must run before `with_cache_dir`: the cache epoch folds in a
        // fingerprint of the user stub set, which `with_cache_dir` only picks
        // up if it's already been configured.
        if let Some(dirs) = self.user_stub_dirs.get()
            && !dirs.is_empty()
        {
            builder = builder.with_user_stubs(Vec::new(), dirs.clone());
        }
        if let Some(dir) = self.session_cache_dir.get() {
            builder = builder.with_cache_dir(dir);
        }
        let session = Arc::new(builder);
        *guard = (php_version, Some(Arc::clone(&session)));
        session
    }

    /// Get-or-build the `AnalysisSession` for the *current* workspace PHP
    /// version — a single lock acquisition on the common (already-built)
    /// path. Replaces `self.analysis_session(self.workspace_php_version())`,
    /// which took two separate locks per call.
    pub fn current_analysis_session(&self) -> Arc<mir_analyzer::AnalysisSession> {
        let guard = self.analysis_session.lock().unwrap();
        if let Some(session) = guard.1.as_ref() {
            return Arc::clone(session);
        }
        let php_version = guard.0;
        drop(guard);
        self.analysis_session(php_version)
    }

    /// Current PHP version tracked by the workspace input.
    pub fn workspace_php_version(&self) -> mir_analyzer::PhpVersion {
        self.analysis_session.lock().unwrap().0
    }

    /// File URIs of all direct and transitive subclasses of `class_fqn`,
    /// resolved via mir's inheritance graph. Returns an empty vec when the mir
    /// session hasn't ingested the class yet (cold start, excluded paths).
    ///
    /// Used by `goto_implementation` and `subtypes` to scope their lookups to
    /// the correct files, fixing aliased `extends` and FQN-qualified forms that
    /// the raw-name `subtypes_of` map misses.
    pub fn class_subtype_urls(&self, class_fqn: &str) -> Vec<tower_lsp_server::ls_types::Uri> {
        let session = self.current_analysis_session();
        session
            .subtype_files(class_fqn)
            .into_iter()
            .filter_map(|p| p.parse::<Uri>().ok())
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
    fn input_durability(uri: &Uri) -> salsa::Durability {
        if uri.as_str().contains("/vendor/") {
            salsa::Durability::HIGH
        } else {
            salsa::Durability::LOW
        }
    }

    /// The `LspWsFile` handle for `uri`, if it is mirrored and not deleted.
    fn lsp_ws_file(&self, uri: &Uri) -> Option<LspWsFile> {
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
        let session = self.current_analysis_session();
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
        let session = self.current_analysis_session();
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
    pub fn mirror_text(&self, uri: &Uri, text: &str) {
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
    pub fn mirror_text_arc(&self, uri: &Uri, text_arc: Arc<str>) {
        let dur = Self::input_durability(uri);
        let path: Arc<str> = Arc::from(uri.as_str());
        let session = self.current_analysis_session();
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
    pub fn source_file(&self, uri: &Uri) -> Option<LspWsFile> {
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
    pub fn seed_cached_index(&self, uri: &Uri, index: Arc<FileIndex>) -> bool {
        let Some(wf) = self.lsp_ws_file(uri) else {
            return false;
        };
        let session = self.current_analysis_session();
        session.with_db_mut(|db| wf.set_cached_index(db).to(Some(index)));
        true
    }

    /// Evict the semantic-tokens cache for `uri`. Called by Backend when a
    /// file is closed; diff-based tokens computed against the old revision
    /// are no longer meaningful.
    pub fn evict_token_cache(&self, uri: &Uri) {
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
        uri: &Uri,
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
    pub fn ingest(&self, uri: Uri, text: &str) {
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
    pub fn ingest_from_doc(&self, uri: Uri, doc: &ParsedDoc) {
        self.mirror_text_arc(&uri, doc.source_arc());
    }

    pub fn remove(&self, uri: &Uri) {
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
        if let Some(session) = guard.1.as_ref() {
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
    pub fn get_doc_salsa(&self, uri: &Uri) -> Option<Arc<ParsedDoc>> {
        self.get_parsed_cached(uri)
    }

    /// Salsa-backed compact symbol index.
    pub fn get_index_salsa(&self, uri: &Uri) -> Option<Arc<FileIndex>> {
        let wf = self.lsp_ws_file(uri)?;
        Some(
            self.snapshot_mir_query(move |db| crate::db::mir_queries::file_index(db, wf).0.clone()),
        )
    }

    /// Salsa-backed pre-computed symbol map (name → Vec<SymbolEntry>).
    /// Memoized per revision: stable files serve from cache in O(1).
    pub fn get_symbol_map_salsa(
        &self,
        uri: &Uri,
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
        uri: &Uri,
        open_urls: &[Uri],
    ) -> Vec<(Uri, Arc<crate::types::symbol_map::SymbolMap>)> {
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
    fn get_parsed_cached(&self, uri: &Uri) -> Option<Arc<ParsedDoc>> {
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
    pub fn get_doc_snapshot_or_stale(&self, uri: &Uri) -> Option<Arc<ParsedDoc>> {
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

        let session = self.current_analysis_session();
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
    /// query invalidation. Returns `true` when the version actually changed —
    /// callers that scan the workspace at runtime (not just at init, before
    /// any file is mirrored) must re-scan every root when this is `true`,
    /// since `drop_session_scoped_state` below empties `lsp_ws_files` and
    /// nothing else repopulates it.
    pub fn set_php_version(&self, version: mir_analyzer::PhpVersion) -> bool {
        {
            let mut guard = self.analysis_session.lock().unwrap();
            if guard.0 == version {
                return false;
            }
            // Clear the cached session too: `current_analysis_session()` trusts
            // `guard.1` unconditionally when present, so leaving the old
            // version's session behind here would silently hand it out under
            // the new version until something calls `analysis_session(version)`
            // explicitly to force the mismatch-triggered rebuild.
            guard.0 = version;
            guard.1 = None;
        }
        // Changing the version selects a different `AnalysisSession` (and thus
        // a different db). In practice the version is set once at init, before
        // any file is mirrored — but a runtime change is possible too (the
        // caller must re-scan in that case, see the `bool` return above).
        self.drop_session_scoped_state();
        true
    }

    /// Discard every piece of state scoped to the current `AnalysisSession`'s
    /// salsa db. MUST accompany anything that causes `analysis_session()` to
    /// hand out a different db (version change, cache-dir attach): the
    /// `LspWsFile` input handles in `lsp_ws_files` index into the *old* db's
    /// tables, and using one against a new db panics with a salsa slot-type
    /// mismatch. The workspace scan re-mirrors on-disk files onto the new
    /// session, but it explicitly skips anything already open in the editor
    /// (so it doesn't clobber live buffer edits with disk content) — callers
    /// whose drop can race an open buffer must re-mirror open files themselves.
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
        let session = self.current_analysis_session();
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
    ///
    /// mir flags a subset of replayed files as untrusted — their postings
    /// carry an unresolved name, so the first live query to touch one pays a
    /// full synchronous `analyze_file` (mir's changelog measured ~1.3-1.5s on
    /// a real 15K-file workspace). That subset is handed to a detached
    /// background thread that reanalyzes them via `reanalyze_files_cancellable`
    /// — the same call the ambient warm sweep uses — so the cost lands during
    /// post-boot idle time instead of a user's first request. Not joined:
    /// this method returns as soon as the replay itself is done, so callers
    /// waiting on it (e.g. the `indexReady` gate) aren't blocked on the
    /// reanalysis too.
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
        let session = self.current_analysis_session();
        let untrusted = session.warm_start_files(&files);
        self.warm_start_replays_completed
            .fetch_add(1, Ordering::Relaxed);
        if untrusted.is_empty() {
            return;
        }
        let counter = Arc::clone(&self.warm_start_untrusted_reanalyzed);
        let spawned = std::thread::Builder::new()
            .name("php-lsp-warm-start-untrusted".into())
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let cancel = mir_analyzer::IndexCancel::new();
                let analyzed = session.reanalyze_files_cancellable(&untrusted, &cancel);
                counter.fetch_add(analyzed.len() as u64, Ordering::Relaxed);
            });
        drop(spawned);
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
            // narrowing is always sound. A builtin (stub-resolved) class is
            // additionally never declared in vendor — PHP core/extension
            // types have no user-authored declaration for any file to
            // legitimately "be" — so vendor mentions of it are dependency-
            // internal noise, same call TypeScript makes for `node_modules`
            // usages of `Promise`.
            mir_analyzer::Name::Class(fqcn) => {
                if let Some(files) = self.fqn_reachable_files(std::slice::from_ref(fqcn)) {
                    return if mir_analyzer::stub_path_for_class(fqcn).is_some() {
                        files.into_iter().filter(|f| !is_vendor_path_str(f)).collect()
                    } else {
                        files
                    };
                }
            }
            // Namespaced functions/constants resolve like class names; an
            // unqualified call to a *global* one (`env()`, `PHP_EOL`) works
            // from any namespace via PHP's global fallback, so FQN
            // reachability can't exclude any file for those. An unqualified
            // *builtin* function is the one case that can still narrow: it's
            // never declared in vendor, so — same reasoning as the builtin
            // class case above — vendor can be dropped outright without an
            // FQN scan.
            mir_analyzer::Name::Function(fqcn) => {
                let trimmed = fqcn.trim_start_matches('\\');
                if trimmed.contains('\\') {
                    if let Some(files) = self.fqn_reachable_files(std::slice::from_ref(fqcn)) {
                        return files;
                    }
                } else if mir_analyzer::is_builtin_function(trimmed) {
                    return self.workspace_file_paths_excluding_vendor();
                }
            }
            // Same split as the function arm: namespaced constants narrow
            // via FQN reachability; an unqualified *builtin* one (`PHP_EOL`)
            // is never declared in vendor, so vendor drops outright.
            mir_analyzer::Name::GlobalConstant(fqcn) => {
                let trimmed = fqcn.trim_start_matches('\\');
                if trimmed.contains('\\') {
                    if let Some(files) = self.fqn_reachable_files(std::slice::from_ref(fqcn)) {
                        return files;
                    }
                } else if mir_analyzer::is_builtin_constant(trimmed) {
                    return self.workspace_file_paths_excluding_vendor();
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
        self.workspace_file_paths_cache
            .store(Some(Arc::clone(&files)));
        files
    }

    /// [`Self::workspace_file_paths`] with vendor files dropped — the
    /// narrowed scope for a symbol that's resolved to something builtin
    /// (never declared in vendor), as opposed to the FQN-reachable narrowing
    /// used for project/vendor-defined symbols.
    fn workspace_file_paths_excluding_vendor(&self) -> Vec<Arc<str>> {
        self.workspace_file_paths()
            .iter()
            .filter(|f| !is_vendor_path_str(f))
            .cloned()
            .collect()
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
        let session = self.current_analysis_session();
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
        let session = self.current_analysis_session();
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
    pub fn method_reference_scope(&self, owner_fqn: &str, method: &str) -> Option<Vec<Uri>> {
        match self.method_reference_scope_plan(owner_fqn, method) {
            MethodScopePlan::Files(files) => Some(files),
            MethodScopePlan::NeedsScan { fqns, extra_needle } => {
                let files =
                    self.fqn_reachable_files_with_needles(&fqns, &[extra_needle.as_str()])?;
                Some(
                    files
                        .iter()
                        .filter_map(|f| (f).parse::<Uri>().ok())
                        .collect(),
                )
            }
            MethodScopePlan::FullWorkspace => None,
        }
    }

    /// Whether [`Self::reference_candidate_files`] narrows `class::method`'s
    /// scope below the full workspace, without paying for the narrowing scan
    /// itself. A caller that further partitions the candidate set by a
    /// heuristic text mention (references' priority-result streaming) only
    /// has anything to gain when the scope is *not* already narrowed — a
    /// narrowed scope is already the minimal necessary set, so partitioning
    /// it further just re-scans those files' bytes for no benefit.
    pub(crate) fn method_scope_is_narrowed(&self, owner_fqn: &str, method: &str) -> bool {
        !matches!(
            self.method_reference_scope_plan(owner_fqn, method),
            MethodScopePlan::FullWorkspace
        )
    }

    /// Same narrowing decision as [`Self::method_reference_scope`], but stops
    /// short of running the FQN/text-needle scan itself — that scan is the
    /// expensive part (a parallel pass over every workspace file), and a
    /// caller resolving many methods at once (code lens, one call per
    /// declaration in a file) needs to batch it across methods via
    /// [`Self::resolve_reachability_queries`] instead of paying it per call.
    fn method_reference_scope_plan(&self, owner_fqn: &str, method: &str) -> MethodScopePlan {
        use crate::index::file_index::{ClassKind, Visibility};

        let ws = self.get_workspace_index_salsa();
        let owner_fqn = owner_fqn.trim_start_matches('\\');
        let owner_short = crate::text::fqn_short_name(owner_fqn);

        let Some((decl_uri, decl_class)) = ws.classes_by_name.get(owner_short).and_then(|refs| {
            refs.iter()
                .filter_map(|&r| ws.at(r))
                .find(|(_, cls)| cls.fqn.trim_start_matches('\\') == owner_fqn)
        }) else {
            // No project/vendor declaration matches this FQN — a builtin
            // owner (`Closure`, `ReflectionParameter`, ...) always lands
            // here, since PHP core/extension classes have no `FileIndex`
            // entry of their own. Same vendor-exclusion reasoning as the
            // `Name::Class`/builtin-function branches in
            // `reference_candidate_files`: a builtin is never declared in
            // vendor, so vendor's own usages of a builtin-owned method are
            // dependency-internal noise.
            if mir_analyzer::stub_path_for_class(owner_fqn).is_some() {
                return MethodScopePlan::Files(
                    self.workspace_file_paths_excluding_vendor()
                        .iter()
                        .filter_map(|f| f.parse::<Uri>().ok())
                        .collect(),
                );
            }
            return MethodScopePlan::FullWorkspace;
        };

        // Trait/interface methods, and methods on classes that compose traits or
        // mixins, can be referenced from other files — keep the full scope.
        if !matches!(decl_class.kind, ClassKind::Class | ClassKind::Enum)
            || !decl_class.traits.is_empty()
            || !decl_class.mixins.is_empty()
        {
            return MethodScopePlan::FullWorkspace;
        }

        let Some(method_def) = decl_class
            .methods
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(method))
        else {
            return MethodScopePlan::FullWorkspace;
        };

        match method_def.visibility {
            Visibility::Private => MethodScopePlan::Files(vec![decl_uri.clone()]),
            Visibility::Protected => {
                if !self.is_index_ready() {
                    return MethodScopePlan::FullWorkspace;
                }
                // Subtype set comes from mir's resolved inheritance graph — it
                // matches subclasses by FQCN, so `extends \Ns\Base` and aliased
                // `use ... as` forms are all found. Falls back to full scope if
                // mir can't resolve the owner.
                let session = self.current_analysis_session();
                let mut files: std::collections::HashSet<Uri> = session
                    .subtype_files(owner_fqn)
                    .into_iter()
                    .filter_map(|p| p.parse::<Uri>().ok())
                    .collect();
                files.insert(decl_uri.clone());
                MethodScopePlan::Files(files.into_iter().collect())
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
            // its short name. Instance methods stay unnarrowed
            // (`FullWorkspace` below): a typed receiver can hide both the
            // class *and* any distinguishing token cheaper than the member
            // name itself.
            Visibility::Public
                if method_def.is_static || method.eq_ignore_ascii_case("__construct") =>
            {
                if !self.is_index_ready() {
                    return MethodScopePlan::FullWorkspace;
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
                let extra_needle = if method.eq_ignore_ascii_case("__construct") {
                    "->__construct".to_string()
                } else {
                    method.to_string()
                };
                MethodScopePlan::NeedsScan { fqns, extra_needle }
            }
            Visibility::Public => MethodScopePlan::FullWorkspace,
        }
    }

    /// Batched form of [`Self::reference_candidate_files`]: resolves the
    /// candidate scope for every symbol in `symbols` while sharing ONE pass
    /// over the workspace for every symbol whose scope needs FQN/text-needle
    /// narrowing (class references, public static methods, constructors),
    /// instead of one pass per symbol.
    ///
    /// Code lens is the motivating caller: a single file's declarations can
    /// produce dozens of reference-count lenses in one request, and each
    /// used to pay its own full `ws.files` scan via
    /// [`Self::fqn_reachable_files_with_needles`] — O(declarations × files)
    /// text scans instead of the one pass this performs.
    ///
    /// Order-preserving: `result[i]` is the scope for `symbols[i]`.
    pub(crate) fn batch_reference_candidate_files(
        &self,
        symbols: &[mir_analyzer::Name],
    ) -> Vec<Vec<Arc<str>>> {
        let mut results: Vec<Option<Vec<Arc<str>>>> = vec![None; symbols.len()];
        let mut queries: Vec<ReachabilityQuery> = Vec::new();
        // queries[k] resolves the scope for symbols[query_targets[k]].
        let mut query_targets: Vec<usize> = Vec::new();

        for (i, symbol) in symbols.iter().enumerate() {
            match symbol {
                mir_analyzer::Name::Method { class, name } => {
                    match self.method_reference_scope_plan(class, name) {
                        MethodScopePlan::Files(urls) => {
                            results[i] = Some(urls.iter().map(|u| Arc::from(u.as_str())).collect());
                        }
                        MethodScopePlan::NeedsScan { fqns, extra_needle } => {
                            queries.push(ReachabilityQuery {
                                fqns,
                                extra_needles: vec![extra_needle],
                            });
                            query_targets.push(i);
                        }
                        MethodScopePlan::FullWorkspace => {}
                    }
                }
                mir_analyzer::Name::Class(fqcn) => {
                    queries.push(ReachabilityQuery {
                        fqns: vec![fqcn.clone()],
                        extra_needles: Vec::new(),
                    });
                    query_targets.push(i);
                }
                mir_analyzer::Name::Function(fqcn) | mir_analyzer::Name::GlobalConstant(fqcn)
                    if fqcn.trim_start_matches('\\').contains('\\') =>
                {
                    queries.push(ReachabilityQuery {
                        fqns: vec![fqcn.clone()],
                        extra_needles: Vec::new(),
                    });
                    query_targets.push(i);
                }
                _ => {}
            }
        }

        if !queries.is_empty()
            && let Some(scanned) = self.resolve_reachability_queries(&queries)
        {
            for (files, target) in scanned.into_iter().zip(query_targets) {
                results[target] = Some(files);
            }
        }

        let fallback = self.workspace_file_paths();
        results
            .into_iter()
            .map(|r| r.unwrap_or_else(|| fallback.as_ref().clone()))
            .collect()
    }

    /// See the `reachability_scan_passes` field's docs. Exposed via
    /// `debugStats` so a protocol-level test can assert a request with many
    /// narrowing-requiring declarations (code lens) pays one scan pass, not
    /// one per declaration.
    pub(crate) fn reachability_scan_passes(&self) -> u64 {
        self.reachability_scan_passes.load(Ordering::Relaxed)
    }

    /// See the `analysis_compute_count` field's docs.
    pub(crate) fn analysis_compute_count(&self) -> u64 {
        self.analysis_compute_count.load(Ordering::Relaxed)
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
    ///
    /// A single-query call through [`Self::resolve_reachability_queries`] —
    /// see that method for the batched form multiple callers share.
    fn fqn_reachable_files_with_needles(
        &self,
        fqns: &[Arc<str>],
        extra_needles: &[&str],
    ) -> Option<Vec<Arc<str>>> {
        let query = ReachabilityQuery {
            fqns: fqns.to_vec(),
            extra_needles: extra_needles.iter().map(|n| n.to_string()).collect(),
        };
        self.resolve_reachability_queries(std::slice::from_ref(&query))
            .map(|mut per_query| per_query.pop().expect("one query in, one result out"))
    }

    /// Resolves many [`Self::fqn_reachable_files_with_needles`]-shaped
    /// queries, memoized per `(query, write_revision)` so a warm repeat
    /// (even for a different needle subset already known to `mention_cache`)
    /// costs a `DashMap` lookup and nothing else.
    ///
    /// This wrapper exists because [`Self::resolve_reachability_queries_uncached`]'s
    /// own `mention_cache` only memoizes the expensive *text scan* — its
    /// outer loop still visits every workspace file to run the cheap
    /// namespace/import checks and a cache lookup, and that "cheap per
    /// file" cost is still measurable multiplied across tens of thousands
    /// of files (confirmed via the `references_all_kinds` bench: ~1.5-2ms
    /// on a 36k-file workspace for query shapes reaching this function,
    /// even on a full `mention_cache` hit). Caching the final result here
    /// skips that loop entirely on a repeat. Queries missing from the cache
    /// are batched into one call to the uncached function (still sharing
    /// one scan pass across every miss, as before).
    fn resolve_reachability_queries(
        &self,
        queries: &[ReachabilityQuery],
    ) -> Option<Vec<Vec<Arc<str>>>> {
        if !self.is_index_ready() {
            return None;
        }
        if queries.is_empty() {
            return Some(Vec::new());
        }
        let revision = self.write_revision.load(Ordering::Acquire);
        let mut out: Vec<Option<Vec<Arc<str>>>> = vec![None; queries.len()];
        let mut miss_queries: Vec<ReachabilityQuery> = Vec::new();
        let mut miss_targets: Vec<usize> = Vec::new();
        for (i, q) in queries.iter().enumerate() {
            match self.reachability_result_cache.get(q) {
                Some(entry) if entry.0 == revision => {
                    out[i] = Some((*entry.1).clone());
                }
                _ => {
                    miss_queries.push(q.clone());
                    miss_targets.push(i);
                }
            }
        }
        if !miss_queries.is_empty() {
            let computed = self.resolve_reachability_queries_uncached(&miss_queries)?;
            let mut cache_files_added = 0u64;
            for (query, result) in miss_queries.into_iter().zip(computed) {
                cache_files_added += result.len() as u64;
                let result = Arc::new(result);
                self.reachability_result_cache
                    .insert(query, (revision, Arc::clone(&result)));
                out[miss_targets.remove(0)] = Some((*result).clone());
            }
            let prior = self
                .reachability_result_cache_files
                .fetch_add(cache_files_added, Ordering::Relaxed);
            const REACHABILITY_RESULT_CACHE_FILE_CAP: u64 = 2_000_000;
            if prior + cache_files_added > REACHABILITY_RESULT_CACHE_FILE_CAP {
                self.reachability_result_cache.clear();
                self.reachability_result_cache_files
                    .store(0, Ordering::Relaxed);
            }
        }
        Some(out.into_iter().map(|o| o.unwrap_or_default()).collect())
    }

    /// Uncached implementation of [`Self::resolve_reachability_queries`],
    /// resolving every query in a single pass over the workspace so N
    /// callers share one scan instead of paying N. Callers should use the
    /// memoizing wrapper.
    ///
    /// The namespace/import checks are cheap in-memory field comparisons and
    /// stay per-query. The qualified-mention fallback is answered by mir's
    /// own `ClassMentionIndex` (`class_mention_scanner`/
    /// `class_mention_answer`/`set_file_class_mentions`) for every needle,
    /// bounded or raw alike — the same persistent per-file mention cache
    /// mir's `indexed_references_to` gate already populates, so a file
    /// scanned by either consumer answers the other for free. A file whose
    /// text and universe-epoch are unchanged since its last scan answers
    /// every needle via a set lookup, no text pass at all. Only a file
    /// that's genuinely edited since its last scan pays the Aho-Corasick
    /// pass, and that pass runs against every currently-known needle in the
    /// universe at once (not just this call's), so the next, differently-
    /// shaped query against that same file is *also* a lookup.
    ///
    /// Returns `None` for every query when the boot scan hasn't finished —
    /// so references are never under-reported at cold start.
    fn resolve_reachability_queries_uncached(
        &self,
        queries: &[ReachabilityQuery],
    ) -> Option<Vec<Vec<Arc<str>>>> {
        use rayon::prelude::*;

        if !self.is_index_ready() {
            return None;
        }
        if queries.is_empty() {
            return Some(Vec::new());
        }
        self.reachability_scan_passes
            .fetch_add(1, Ordering::Relaxed);

        // Per-query targets/namespaces for the cheap checks, plus this
        // query's final needle strings — the exact literal form fed to the
        // universe/scanner (a bare short name carries its `\` prefix).
        let mut per_query_targets: Vec<Vec<&str>> = Vec::with_capacity(queries.len());
        let mut per_query_namespaces: Vec<Vec<Option<&str>>> = Vec::with_capacity(queries.len());
        let mut per_query_needles: Vec<Vec<String>> = Vec::with_capacity(queries.len());

        for q in queries {
            let targets: Vec<&str> = q.fqns.iter().map(|f| f.trim_start_matches('\\')).collect();
            let namespaces: Vec<Option<&str>> = targets
                .iter()
                .map(|t| t.rsplit_once('\\').map(|(ns, _)| ns))
                .collect();
            // Namespaced targets match with or without the leading `\`; a
            // bare global name keeps it (the short name alone would match
            // everything).
            let mut needles: Vec<String> = targets
                .iter()
                .map(|t| {
                    if t.contains('\\') {
                        (*t).to_string()
                    } else {
                        format!("\\{t}")
                    }
                })
                .collect();
            needles.extend(q.extra_needles.iter().cloned());
            per_query_targets.push(targets);
            per_query_namespaces.push(namespaces);
            per_query_needles.push(needles);
        }

        // Every needle is answered by mir's `ClassMentionIndex` — the same
        // persistent per-file mention cache mir's own `indexed_references_to`
        // gate already populates, so a file scanned by either consumer
        // answers the other for free. A needle that's a whole identifier or
        // FQN literal (alphanumeric/underscore/backslash only) gets the
        // stricter, more precise word-boundary match (and is sound: PHP's
        // lexer never places a qualified name flush against another
        // identifier with no separator — `new App\Foo\Bar()` requires that
        // space, `newApp\Foo\Bar` would lex as one invalid token — so the
        // boundary check never under-reports a literal FQN mention in valid
        // source). A needle that isn't a whole identifier (a call token
        // like `->__construct`, whose preceding byte in real usage —
        // `$obj->__construct()` — is itself an identifier character and
        // would otherwise fail that check) is admitted as a raw needle
        // instead, matched with no boundary at all — see
        // `mir_analyzer::db::ClassMentionIndex::add_raw_names`'s doc
        // comment.
        fn needle_needs_raw_match(s: &str) -> bool {
            !s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '\\')
        }

        // `snapshot_db` below is dropped before `get_workspace_index_salsa`
        // runs — that function takes its own snapshot internally, and a
        // concurrent writer's `set()` holds mir's write lock waiting for
        // *every* outstanding snapshot to drop while a fresh `snapshot_db`
        // waits for the read lock behind that same writer; keeping this
        // snapshot alive across the call is exactly the deadlock
        // `Self::snapshot_mir_query`'s doc comment warns about. The
        // parallel loop below takes its own fresh snapshot instead of
        // reusing this one.
        {
            let mir_db = self.current_analysis_session().snapshot_db();
            let all_needles = per_query_needles.iter().flatten().map(|s| s.as_str());
            let (raw_needles, bounded_needles): (Vec<&str>, Vec<&str>) =
                all_needles.partition(|n| needle_needs_raw_match(n));
            if !bounded_needles.is_empty() {
                mir_db.add_literal_mention_names(bounded_needles);
            }
            if !raw_needles.is_empty() {
                mir_db.add_raw_mention_needles(raw_needles);
            }
        }

        let ws = self.get_workspace_index_salsa();
        let n = queries.len();

        let mir_db = self.current_analysis_session().snapshot_db();
        let mir_scanner = mir_db.class_mention_scanner();
        let per_query_mir_queries: Vec<Vec<mir_analyzer::db::MentionQuery>> = per_query_needles
            .iter()
            .map(|needles| {
                needles
                    .iter()
                    .filter_map(|n| mir_db.prepare_class_mention_query(n))
                    .collect()
            })
            .collect();
        // For a common short name most of the corpus fails the cheap
        // namespace/import checks and falls through to the mention lookup
        // (or, for a file that's cold or edited since its last scan, the
        // text scan that fills it in) — parallel for the same reason as
        // before: mir's own equivalent gate scan does the same, and a cold
        // query on a 15K-file workspace needs the few-ms parallel pass, not
        // a multi-second sequential one.
        let per_file_matches: Vec<(Arc<str>, Vec<bool>)> = ws
            .files
            .par_iter()
            .map_with(mir_db.clone(), |mir_db, (url, idx)| {
                let file_ns = idx.namespace.as_deref().map(|s| s.trim_start_matches('\\'));
                let mut matched = vec![false; n];
                let mut unresolved: Vec<usize> = Vec::new();
                for qi in 0..n {
                    let ns_match = per_query_namespaces[qi]
                        .iter()
                        .any(|ns| match (file_ns, ns) {
                            (Some(f), Some(t)) => fqn_segment_prefix(f, t),
                            (None, None) => true,
                            // Global-ns files reach namespaced targets only via
                            // the full path (text rule); namespaced files reach
                            // global classes only via `\Name` or an import.
                            _ => false,
                        });
                    if ns_match {
                        matched[qi] = true;
                        continue;
                    }
                    let import_match = idx.use_imports.iter().any(|(_, f)| {
                        let f = f.trim_start_matches('\\');
                        per_query_targets[qi]
                            .iter()
                            .any(|t| fqn_segment_prefix(f, t))
                    });
                    if import_match {
                        matched[qi] = true;
                        continue;
                    }
                    unresolved.push(qi);
                }
                if !unresolved.is_empty()
                    && let Some(text) = self.source_text(url)
                {
                    let url_str = url.as_str();
                    let mut needs_scan = false;
                    for &qi in &unresolved {
                        for q in &per_query_mir_queries[qi] {
                            match mir_db.class_mention_answer(url_str, q, &text) {
                                Some(true) => {
                                    matched[qi] = true;
                                    break;
                                }
                                Some(false) => {}
                                None => {
                                    needs_scan = true;
                                }
                            }
                        }
                    }
                    if needs_scan
                        && let Some(mir_scanner) = &mir_scanner
                    {
                        // Scanned against the *whole* current universe, not
                        // just this call's needles, so a future, differently-
                        // shaped query against this same file is also a
                        // lookup instead of its own fresh scan.
                        let names = mir_scanner.scan(text.as_ref());
                        for &qi in &unresolved {
                            if !matched[qi]
                                && per_query_mir_queries[qi]
                                    .iter()
                                    .any(|q| names.binary_search(&q.name).is_ok())
                            {
                                matched[qi] = true;
                            }
                        }
                        mir_db.set_file_class_mentions(
                            &Arc::<str>::from(url_str),
                            &text,
                            mir_scanner.epoch(),
                            names,
                        );
                    }
                }
                matched
                    .iter()
                    .any(|&m| m)
                    .then(|| (Arc::<str>::from(url.as_str()), matched))
            })
            .flatten()
            .collect();

        let mut out: Vec<Vec<Arc<str>>> = vec![Vec::new(); n];
        for (file, matched) in per_file_matches {
            for (qi, out_files) in out.iter_mut().enumerate() {
                if matched[qi] {
                    out_files.push(file.clone());
                }
            }
        }
        Some(out)
    }

    /// Phase J: salsa-memoized aggregate workspace index.
    ///
    /// Returns the shared `Arc<WorkspaceIndexData>` with flat
    /// `(Uri, Arc<FileIndex>)` list plus pre-built `classes_by_name` and
    /// `subtypes_of` reverse maps. Used by workspace_symbols,
    /// prepare_type_hierarchy, supertypes_of, subtypes_of, and
    /// find_implementations so they don't each rebuild the aggregate per
    /// request. Invalidates automatically when any file's `file_index`
    /// changes.
    pub fn get_workspace_index_salsa(&self) -> Arc<crate::db::workspace_index::WorkspaceIndexData> {
        self.sync_workspace_files();
        let ws = *self.lsp_workspace.lock().unwrap();
        let Some(ws) = ws else {
            return Arc::new(crate::db::workspace_index::WorkspaceIndexData::from_files(
                Vec::new(),
            ));
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
        self.current_analysis_session().ref_index_lock_count()
    }

    /// Diagnostic: mir's `indexed_references_to` memoization hits, surfaced
    /// via `$/php-lsp/debugStats` so a host-side query can be attributed to
    /// the right layer when diagnosing latency.
    pub fn mir_ref_query_cache_hits(&self) -> u64 {
        self.current_analysis_session().ref_query_cache_hits()
    }

    /// Diagnostic: mir's `indexed_subtype_classes` memoization hits.
    pub fn mir_subtype_query_cache_hits(&self) -> u64 {
        self.current_analysis_session().subtype_query_cache_hits()
    }

    /// Whether mir's workspace symbol index singleton is populated (warm-start
    /// seed or first sweep). See `DebugStats::workspace_symbol_index_ready`.
    pub fn workspace_symbol_index_ready(&self) -> bool {
        self.analysis_session(self.workspace_php_version())
            .workspace_symbol_index_ready()
    }

    /// Executions of mir's tracked O(all-files) symbol-index walk.
    pub fn workspace_index_walks(&self) -> u64 {
        self.analysis_session(self.workspace_php_version())
            .workspace_index_walks()
    }

    /// Number of files mirrored into the salsa workspace (open + background
    /// indexed). Surfaced via `$/php-lsp/debugStats` as a denominator for the
    /// per-file cache sizes below — e.g. `text_cache_len` tracking this 1:1
    /// means it's carrying the workspace's working set, not leaking.
    pub fn workspace_file_count(&self) -> u64 {
        self.lsp_ws_files.len() as u64
    }

    /// Entries in the semantic-tokens delta cache. Evicted per-file on
    /// `did_close`; a value that keeps climbing past the number of files
    /// ever opened in the editor means that eviction isn't firing.
    pub fn token_cache_len(&self) -> u64 {
        self.caches.token_cache.len() as u64
    }

    /// Entries in the mirrored-source-text cache. Expected to track
    /// [`Self::workspace_file_count`] — it shares its `Arc<str>` with
    /// salsa's own `SourceFile::text`, so this is a pointer per file, not a
    /// second copy of the workspace's source.
    pub fn text_cache_len(&self) -> u64 {
        self.caches.text_cache.len() as u64
    }

    /// Entries in the read-through `ParsedDoc` cache. Bounded by
    /// `PARSED_CACHE_CAP` via LRU shedding.
    pub fn parsed_cache_len(&self) -> u64 {
        self.caches.parsed_cache.len() as u64
    }

    /// Entries in the per-file `FileAnalysis` cache. Bounded by
    /// `ANALYSIS_CACHE_CAP` via LRU shedding.
    pub fn analysis_cache_len(&self) -> u64 {
        self.caches.analysis_cache.len() as u64
    }

    /// Entries in the owned-`Program` cache. Bounded by
    /// `OWNED_PROGRAM_CACHE_CAP` via LRU shedding.
    pub fn owned_program_cache_len(&self) -> u64 {
        self.caches.owned_program_cache.len() as u64
    }

    /// Entries in the declaration-fingerprint cache — one per file that
    /// declares something, used to detect cross-file declaration changes.
    /// Expected to track [`Self::workspace_file_count`], not grow past it.
    pub fn decl_fingerprints_len(&self) -> u64 {
        self.caches.decl_fingerprints.len() as u64
    }

    /// Entries in the lazily-loaded vendor `FileIndex` cache, populated by
    /// PSR-4 "go to definition" navigation into `vendor/`. Unlike the other
    /// per-file caches above, this one has no LRU cap today — a session with
    /// heavy navigation through a large dependency tree will keep growing
    /// this for the life of the process.
    pub fn vendor_index_cache_len(&self) -> u64 {
        self.caches.vendor_index_cache.len() as u64
    }

    /// Per-file text scans recorded in mir's own `ClassMentionIndex` — the
    /// single shared backend every [`Self::resolve_reachability_queries`]
    /// needle is answered by, whether it's a fully-qualified literal or a
    /// raw call-token needle. The ratio against `reachability_scan_passes`
    /// is the per-file mention cache's hit rate; a warm, unedited repeat
    /// query should bump `reachability_scan_passes` without bumping this at
    /// all.
    pub(crate) fn mir_mention_scans_recorded(&self) -> u64 {
        self.current_analysis_session()
            .class_mention_stats()
            .scans_recorded
    }

    /// Return the raw source text for `uri` if it has been mirrored into the
    /// salsa workspace. Used by the references handler to pre-filter session
    /// results by checking whether a file mentions the owning class name.
    pub fn source_text(&self, uri: &Uri) -> Option<Arc<str>> {
        self.caches.text_cache.get(uri).map(|e| Arc::clone(&e))
    }

    /// Cache the semantic tokens computed for a delta response.
    /// `result_id` is an opaque string (a hash of the token data) returned to the client.
    pub fn store_token_cache(&self, uri: &Uri, result_id: String, tokens: Arc<Vec<SemanticToken>>) {
        self.caches.store_token(uri, result_id, tokens);
    }

    /// Return the cached tokens if `result_id` matches the stored one.
    pub fn get_token_cache(&self, uri: &Uri, result_id: &str) -> Option<Arc<Vec<SemanticToken>>> {
        self.caches.get_token(uri, result_id)
    }

    /// Raw semantic issues for a file, computed via mir's session-based
    /// `FileAnalyzer`. The session lazy-loads dependencies via PSR-4 so the
    /// LSP no longer needs to mirror vendor up-front. Callers apply their
    /// own `DiagnosticsConfig` filter via
    /// [`crate::semantic_diagnostics::issues_to_diagnostics`].
    #[tracing::instrument(skip_all)]
    pub fn get_semantic_issues_salsa(&self, uri: &Uri) -> Option<Arc<[mir_issues::Issue]>> {
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
                let session = self.current_analysis_session();
                if let Ok(issues) = salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                    session.class_issues(std::slice::from_ref(&file))
                })) {
                    break issues;
                }
            }
        };
        // `collector_issues` includes raw `ParseError` issues; callers filter
        // those out downstream in `issues_to_diagnostics` (php-lsp already
        // surfaces parse errors as `SyntaxError` diagnostics from its own
        // parser pass), so this returns them unfiltered here.
        let collector_issues = {
            let session = self.current_analysis_session();
            session.collector_issues(std::slice::from_ref(&file))
        };
        let combined: Vec<mir_issues::Issue> = analysis
            .issues
            .iter()
            .cloned()
            .chain(class_issues)
            .chain(collector_issues)
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
    pub fn cached_analysis_if_fresh(&self, uri: &Uri) -> Option<Arc<mir_analyzer::FileAnalysis>> {
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

    pub fn cached_analysis(&self, uri: &Uri) -> Option<Arc<mir_analyzer::FileAnalysis>> {
        self.cached_analysis_cancellable(uri, &|| false)
    }

    /// Compare `uri`'s current `FileIndex` against its stored declaration
    /// fingerprint, bumping `decl_version` (and `session`'s prepare
    /// generation) when declarations changed or this is the file's
    /// first-seen fingerprint. Body-only edits leave the counter unchanged so
    /// sibling files keep serving from cache. Returns whether it bumped.
    fn sync_decl_fingerprint(&self, uri: &Uri, session: &mir_analyzer::AnalysisSession) -> bool {
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
        decl_changed
    }

    /// Register a newly-discovered file's declarations so a consumer that was
    /// analyzed *before* this file existed doesn't keep a stale cached
    /// analysis forever.
    ///
    /// `mirror_text`/`ingest_from_doc` alone never bump `decl_version` —
    /// that only happens inside `cached_analysis_cancellable`'s own
    /// first-analysis check, which runs when a file undergoes its *own* full
    /// analysis. A file that is merely scanned or created as someone else's
    /// dependency (what a workspace scan or a `didChangeWatchedFiles`
    /// CREATED/CHANGED event does) never goes through that path, so without
    /// this call, a consumer analyzed earlier keeps returning a stale
    /// `UndefinedClass` (or similar) even after the dependency shows up.
    ///
    /// Call this after mirroring a file from one of those discovery paths —
    /// not from every `mirror_text` call, which would also fire on every
    /// keystroke edit to an already-open file and defeat the whole point of
    /// caching sibling files' analyses across those edits.
    pub fn note_new_file_declarations(&self, uri: &Uri) {
        let session = self.current_analysis_session();
        self.sync_decl_fingerprint(uri, &session);
    }

    /// [`Self::cached_analysis`] with an early exit: `should_cancel` is
    /// polled whenever a concurrent salsa write cancels the analysis attempt.
    /// A handler passes its write-revision staleness probe so a request made
    /// obsolete by newer typing stops burning a blocking-pool thread instead
    /// of retrying until the writer pauses — the editor re-requests anyway.
    #[tracing::instrument(skip_all)]
    pub fn cached_analysis_cancellable(
        &self,
        uri: &Uri,
        should_cancel: &(dyn Fn() -> bool + Sync),
    ) -> Option<Arc<mir_analyzer::FileAnalysis>> {
        // Need the parsed doc both for the analyzer and as the cache key.
        let doc = self.get_doc_salsa(uri)?;
        let source = doc.source_arc();

        if let Some(hit) = self.cached_analysis_if_fresh(uri) {
            return Some(hit);
        }

        // Serialize concurrent callers analyzing the SAME uncached file onto
        // one computation instead of each redoing the full mir pass — e.g.
        // `did_open`'s diagnostics trigger racing a fast-following hover on a
        // large just-opened file (tower-lsp's default concurrency runs up to
        // 4 in-flight LSP messages at once, so this genuinely happens). Other
        // files never contend: the lock is per-URI.
        let inflight = Arc::clone(
            self.analysis_inflight
                .entry(uri.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .value(),
        );
        let _inflight_guard = inflight.lock().unwrap();

        // Re-check now that we hold the per-URI lock: whoever we waited on
        // may have just populated the cache. `cur_ver` becomes the freshness
        // tag on OUR insert below, so re-fetching it here (rather than reusing
        // a value captured before we waited) reflects what was actually true
        // right before we started computing, not before we started waiting.
        if let Some(hit) = self.cached_analysis_if_fresh(uri) {
            return Some(hit);
        }
        let cur_ver = self.caches.decl_version();

        let session = self.current_analysis_session();
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
                Ok(a) => {
                    self.analysis_compute_count.fetch_add(1, Ordering::Relaxed);
                    break Arc::new(a);
                }
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
        let decl_changed = self.sync_decl_fingerprint(uri, &session);
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
    pub fn docs_for(&self, open_urls: &[Uri]) -> Vec<(Uri, Arc<ParsedDoc>)> {
        open_urls
            .iter()
            .filter_map(|u| self.get_doc_salsa(u).map(|d| (u.clone(), d)))
            .collect()
    }

    /// Parsed docs for every entry in `open_urls` except `uri`.
    pub fn other_docs(&self, uri: &Uri, open_urls: &[Uri]) -> Vec<(Uri, Arc<ParsedDoc>)> {
        open_urls
            .iter()
            .filter(|u| *u != uri)
            .filter_map(|u| self.get_doc_salsa(u).map(|d| (u.clone(), d)))
            .collect()
    }

    /// Compact symbol index for every mirrored file.
    pub fn all_indexes(&self) -> Vec<(Uri, Arc<FileIndex>)> {
        self.get_workspace_index_salsa().files.clone()
    }

    /// Borrow-scoped alternative to `all_indexes()` for callers that only
    /// need the slice for the duration of one synchronous call — avoids
    /// cloning every `Uri` in the aggregate (`get_workspace_index_salsa()`
    /// itself is a cheap `Arc` clone; `all_indexes()`'s `.files.clone()` is
    /// the expensive part). Use `all_indexes()` instead when the result must
    /// be moved across an `.await`/`spawn_blocking` boundary.
    pub fn with_all_indexes<R>(&self, f: impl FnOnce(&[(Uri, Arc<FileIndex>)]) -> R) -> R {
        f(&self.get_workspace_index_salsa().files)
    }

    /// Store a lazily-loaded vendor `FileIndex` in the session cache.
    /// Only call this for files that are not part of the normal workspace scan
    /// (i.e. vendor files loaded on-demand by PSR-4 navigation).
    pub fn cache_vendor_index(&self, uri: Uri, index: Arc<FileIndex>) {
        self.caches.vendor_index_cache.insert(uri, index);
    }

    /// Retrieve a previously cached vendor `FileIndex`.
    pub fn get_vendor_index(&self, uri: &Uri) -> Option<Arc<FileIndex>> {
        self.caches
            .vendor_index_cache
            .get(uri)
            .map(|e| Arc::clone(&*e))
    }

    /// Same as `all_indexes` but excludes `uri`.
    pub fn other_indexes(&self, uri: &Uri) -> Vec<(Uri, Arc<FileIndex>)> {
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
    pub fn all_docs_for_scan(&self) -> Vec<(Uri, Arc<ParsedDoc>)> {
        let urls: Vec<Uri> = self
            .lsp_ws_files
            .iter()
            .filter(|e| !self.deleted_uris.contains(e.key()))
            .map(|e| e.key().clone())
            .collect();
        urls.into_iter()
            .filter_map(|u| self.get_doc_salsa(&u).map(|d| (u, d)))
            .collect()
    }

    /// Like [`Self::all_docs_for_scan`], but only parses files whose raw text
    /// mentions at least one of `needles` as a whole identifier
    /// (ASCII-case-insensitive — PHP class names are case-insensitive). Used
    /// by callers scanning for a class/interface *declaration* by name (e.g.
    /// "implement missing methods"), where the needle set is small and a miss
    /// guarantees the file declares none of them.
    ///
    /// Answered from mir's persistent per-file `ClassMentionIndex` — the
    /// same cache the reference/subtype gates and
    /// [`Self::resolve_reachability_queries`] populate — so a file scanned
    /// here answers later reference queries for free, and vice versa. Text
    /// comes from this store's own mirror (`source_text`), same as before;
    /// `class_mention_answer` validates cache freshness against it, so a
    /// file whose mirror diverges from mir's copy is simply rescanned.
    pub fn docs_for_scan_mentioning(&self, needles: &[String]) -> Vec<(Uri, Arc<ParsedDoc>)> {
        if needles.is_empty() {
            return Vec::new();
        }
        // Scoped so the mir snapshot drops before `get_doc_salsa` below —
        // same writer-starvation discipline as
        // `resolve_reachability_queries_uncached`.
        let urls: Vec<Uri> = {
            let mir_db = self.current_analysis_session().snapshot_db();
            mir_db.add_literal_mention_names(needles.iter().map(|s| s.as_str()));
            let queries: Vec<mir_analyzer::db::MentionQuery> = needles
                .iter()
                .filter_map(|n| mir_db.prepare_class_mention_query(n))
                .collect();
            if queries.is_empty() {
                return Vec::new();
            }
            let scanner = mir_db.class_mention_scanner();
            self.lsp_ws_files
                .iter()
                .filter(|e| !self.deleted_uris.contains(e.key()))
                .filter(|e| {
                    let Some(text) = self.source_text(e.key()) else {
                        return false;
                    };
                    let url_str = e.key().as_str();
                    let mut unanswered = false;
                    for q in &queries {
                        match mir_db.class_mention_answer(url_str, q, &text) {
                            Some(true) => return true,
                            Some(false) => {}
                            None => unanswered = true,
                        }
                    }
                    if !unanswered {
                        return false;
                    }
                    let Some(scanner) = &scanner else {
                        // Defensive only: the needles were admitted above,
                        // so the universe (and scanner) can't be empty.
                        return needles.iter().any(|n| {
                            crate::text::contains_ascii_case_insensitive(&text, n)
                        });
                    };
                    let names = scanner.scan(text.as_ref());
                    let hit = queries
                        .iter()
                        .any(|q| names.binary_search(&q.name).is_ok());
                    mir_db.set_file_class_mentions(
                        &Arc::<str>::from(url_str),
                        &text,
                        scanner.epoch(),
                        names,
                    );
                    hit
                })
                .map(|e| e.key().clone())
                .collect()
        };
        urls.into_iter()
            .filter_map(|u| self.get_doc_salsa(&u).map(|d| (u, d)))
            .collect()
    }

    /// Subset of `files` (mir paths) mentioning `short_name` as a whole
    /// identifier, ASCII-case-insensitive, via mir's shared per-file mention
    /// cache (`AnalysisSession::files_mentioning_any`). A file mir doesn't
    /// know is omitted — callers use this as a prioritization heuristic, not
    /// an authoritative filter.
    pub fn files_mentioning_short_name(
        &self,
        files: &[Arc<str>],
        short_name: &str,
    ) -> Vec<Arc<str>> {
        self.current_analysis_session()
            .files_mentioning_any(files, &[short_name])
    }

    /// Files whose `use` imports include `fqn` (leading `\` and ASCII case
    /// ignored — PHP names are case-insensitive), from the workspace symbol
    /// index — no parsing, no text scan. The candidate scope for `use`-line
    /// rewrites on file rename/delete: only importers can carry such a line.
    pub fn files_importing(&self, fqn: &str) -> Vec<Uri> {
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

/// `uri`'s path has a `vendor` component, e.g. `file:///proj/vendor/acme/Lib.php`
/// or the nested-monorepo `file:///proj/packages/api/vendor/x/Y.php`. URI
/// paths are always `/`-separated regardless of platform, so a plain
/// component split is enough — no OS path handling needed.
fn is_vendor_uri(uri: &Uri) -> bool {
    is_vendor_path_str(uri.path().as_str())
}

/// [`is_vendor_uri`] on a raw URI/path string, for callers already holding
/// one (e.g. the `Arc<str>` candidate lists `reference_candidate_files`
/// deals in) instead of a parsed [`Uri`].
fn is_vendor_path_str(s: &str) -> bool {
    s.split('/').any(|seg| seg == "vendor")
}

/// The narrowing decision for a method's reference scope, computed without
/// running the expensive part (the FQN/text-needle workspace scan) so a
/// batch caller can pool that scan across many methods.
enum MethodScopePlan {
    /// Fully resolved already — no workspace scan needed at all (private:
    /// the declaring file alone; protected: the declaring file plus its
    /// resolved subtype set).
    Files(Vec<Uri>),
    /// A public static method or constructor: resolvable to the declaring
    /// class's FQN-reachable files unioned with files matching one extra
    /// text needle. This is the case a batch caller pools.
    NeedsScan {
        fqns: Vec<Arc<str>>,
        extra_needle: String,
    },
    /// No narrowing possible (instance methods, an unresolvable owner, or
    /// the boot scan hasn't finished) — full workspace scope.
    FullWorkspace,
}

/// One [`DocumentStore::resolve_reachability_queries`] request: the target
/// FQNs for the namespace/import rules, plus raw text needles matched the
/// same way (ASCII-case-insensitive substring).
#[derive(Clone, PartialEq, Eq, Hash)]
struct ReachabilityQuery {
    fqns: Vec<Arc<str>>,
    extra_needles: Vec<String>,
}

/// Memoized [`DocumentStore::resolve_reachability_queries`] results: query
/// -> `(write_revision computed at, resolved file list)`. See the field doc
/// on `DocumentStore::reachability_result_cache`.
type ReachabilityResultCache = DashMap<ReachabilityQuery, (u64, Arc<Vec<Arc<str>>>)>;

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Uri {
        format!("file://{path}").parse::<Uri>().unwrap()
    }

    /// Phase E4: open-file state lives on `Backend`, not `DocumentStore`.
    /// Tests that need to simulate "file is open" just mirror the text into
    /// the salsa input — the open/closed distinction is enforced by the
    /// caller (Backend) in production.
    fn open(store: &DocumentStore, u: Uri, text: String) {
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

        let session = store.current_analysis_session();
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
        let uris: Vec<Uri> = (0..20).map(|i| uri(&format!("/churn/f{i}.php"))).collect();
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
        expect_test::expect![[r#"
            [
                (
                    "file:///caller.php",
                    4,
                    43,
                    46,
                ),
            ]
        "#]]
        .assert_debug_eq(&cold);

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
        expect_test::expect![[r#"
            [
                (
                    "file:///caller.php",
                    4,
                    43,
                    46,
                ),
                (
                    "file:///caller.php",
                    4,
                    60,
                    63,
                ),
            ]
        "#]]
        .assert_debug_eq(&after_edit);
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
    fn sweep_candidate_files_excludes_vendor_from_the_ambient_tail() {
        let store = DocumentStore::new();
        store.ingest(
            uri("/vendor/acme/Lib.php"),
            "<?php\nnamespace Acme;\nclass Lib {}",
        );
        store.ingest(uri("/src/Own.php"), "<?php\nnamespace App;\nclass Own {}");
        store.mark_index_ready();

        let files = store.sweep_candidate_files(&[]);
        let files: Vec<&str> = files.iter().map(|f| f.as_ref()).collect();
        assert!(
            !files.contains(&uri("/vendor/acme/Lib.php").as_str()),
            "vendor file must not be in the ambient (non-priority) sweep list: {files:?}"
        );
        assert!(
            files.contains(&uri("/src/Own.php").as_str()),
            "non-vendor file must still be in the ambient sweep list: {files:?}"
        );
    }

    #[test]
    fn sweep_candidate_files_still_includes_an_explicitly_prioritized_vendor_file() {
        let store = DocumentStore::new();
        store.ingest(
            uri("/vendor/acme/Lib.php"),
            "<?php\nnamespace Acme;\nclass Lib {}",
        );
        store.mark_index_ready();

        // Opening the vendor file directly makes it a priority target, which
        // bypasses the ambient-sweep vendor exclusion.
        let files = store.sweep_candidate_files(&[uri("/vendor/acme/Lib.php")]);
        let files: Vec<&str> = files.iter().map(|f| f.as_ref()).collect();
        assert!(
            files.contains(&uri("/vendor/acme/Lib.php").as_str()),
            "an explicitly opened/prioritized vendor file must still be warmed: {files:?}"
        );
    }

    #[test]
    fn is_vendor_uri_matches_top_level_and_nested_vendor_dirs_only() {
        assert!(is_vendor_uri(&uri("/proj/vendor/acme/Lib.php")));
        assert!(is_vendor_uri(&uri(
            "/proj/packages/api/vendor/acme/Lib.php"
        )));
        assert!(!is_vendor_uri(&uri("/proj/src/Vendored/Lib.php")));
        assert!(!is_vendor_uri(&uri("/proj/src/VendorLib.php")));
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
        let indexes = store.all_indexes();
        let mut uris: Vec<&str> = indexes.iter().map(|(u, _)| u.as_str()).collect();
        uris.sort();
        assert_eq!(uris, vec![uri("/a.php").as_str(), uri("/b.php").as_str()]);
    }

    #[test]
    fn other_indexes_excludes_current_uri() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nfunction a() {}".to_string());
        open(&store, uri("/b.php"), "<?php\nfunction b() {}".to_string());
        let others = store.other_indexes(&uri("/a.php"));
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].0.as_str(), uri("/b.php").as_str());
    }

    #[test]
    fn other_docs_excludes_current_uri() {
        let store = DocumentStore::new();
        let ua = uri("/a.php");
        let ub = uri("/b.php");
        open(&store, ua.clone(), "<?php\nfunction a() {}".to_string());
        open(&store, ub.clone(), "<?php\nfunction b() {}".to_string());
        let open_urls = vec![ua.clone(), ub.clone()];
        let others = store.other_docs(&ua, &open_urls);
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].0.as_str(), ub.as_str());
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
        let cached = store.get_vendor_index(&u).unwrap();
        assert!(
            Arc::ptr_eq(&idx, &cached),
            "get_vendor_index must return the exact FileIndex that was cached"
        );
        store.remove(&u);
        assert!(store.get_vendor_index(&u).is_none());
    }

    /// A repeat reachability query against unchanged files must not re-scan
    /// any of them. `fqn_reachable_files` has no raw `extra_needles`, so
    /// it's answered by mir's `ClassMentionIndex` (see
    /// `resolve_reachability_queries_uncached`'s doc comment) —
    /// `mir_mention_scans_recorded` (bumped only on an actual text scan
    /// inside that index) proves this directly rather than inferring it
    /// from timing.
    #[test]
    fn reachability_repeat_query_hits_mention_cache() {
        let store = DocumentStore::new();
        for i in 0..5 {
            open(
                &store,
                uri(&format!("/Other/Noise{i}.php")),
                format!(
                    "<?php\nnamespace Other{i};\nclass N{i} {{ public function run() {{ return new \\App\\Owner(); }} }}\n"
                ),
            );
        }
        store.mark_index_ready();

        let target: Arc<str> = Arc::from("App\\Owner");
        let first = store
            .fqn_reachable_files(&[target.clone()])
            .expect("index is ready");
        assert_eq!(first.len(), 5, "every noise file textually mentions the target");
        let scans_after_first = store.mir_mention_scans_recorded();
        assert!(
            scans_after_first > 0,
            "cold query must scan the noise files' text at least once"
        );

        let second = store
            .fqn_reachable_files(&[target])
            .expect("index is ready");
        assert_eq!(second.len(), 5);
        assert_eq!(
            store.mir_mention_scans_recorded(),
            scans_after_first,
            "identical repeat query must not re-scan any file's text"
        );
    }

    /// A needle admitted into the universe *after* a file was last scanned
    /// must still be found on that file — the per-file `epoch` check must
    /// force a rescan rather than answering from a mention-set that
    /// predates the needle. Without this, mir's `ClassMentionIndex`'s
    /// "append-only, lazily grown" design would silently under-report
    /// references to any symbol first queried after another symbol already
    /// warmed the cache for the same file.
    #[test]
    fn reachability_new_needle_still_finds_previously_scanned_file() {
        let store = DocumentStore::new();
        open(
            &store,
            uri("/Other/Multi.php"),
            "<?php\nnamespace Other;\nclass Multi { public function run() {\n    new \\App\\First();\n    new \\App\\Second();\n} }\n"
                .to_string(),
        );
        store.mark_index_ready();

        // First query only asks about `First` — this forces a scan of
        // Multi.php's text, caching its mention-set at an epoch that
        // includes `First` but not yet `Second`.
        let first_hits = store
            .fqn_reachable_files(&[Arc::from("App\\First")])
            .expect("index is ready");
        assert_eq!(first_hits.len(), 1);

        // Second query: a brand-new needle never admitted before.
        // Multi.php's cached entry predates it.
        let second_hits = store
            .fqn_reachable_files(&[Arc::from("App\\Second")])
            .expect("index is ready");
        assert_eq!(
            second_hits.len(),
            1,
            "a needle admitted after a file's last scan must still be found on that file"
        );
    }

    /// An edit that adds a new mention must not be served from a stale
    /// cache entry — `Arc::ptr_eq` against the cached `text` must detect
    /// the change and force a fresh scan.
    #[test]
    fn reachability_cache_invalidates_on_edit() {
        let store = DocumentStore::new();
        let target: Arc<str> = Arc::from("App\\Owner");
        let noise = uri("/Other/Noise.php");
        open(
            &store,
            noise.clone(),
            "<?php\nnamespace Other;\nclass Noise {}\n".to_string(),
        );
        store.mark_index_ready();

        let before = store
            .fqn_reachable_files(&[target.clone()])
            .expect("index is ready");
        assert!(before.is_empty(), "file does not mention the target yet");

        open(
            &store,
            noise,
            "<?php\nnamespace Other;\nclass Noise { public function run() { return new \\App\\Owner(); } }\n"
                .to_string(),
        );

        let after = store
            .fqn_reachable_files(&[target])
            .expect("index is ready");
        assert_eq!(
            after.len(),
            1,
            "an edit that adds a mention must not be served from a stale cache entry"
        );
    }

    /// A repeat query must not even enter `resolve_reachability_queries_uncached`
    /// — `mention_cache` (proven above) only skips the text scan *inside*
    /// that function, but its outer per-file loop still runs on every call
    /// unless the result itself is cached. `reachability_scan_passes` (bumped
    /// only inside the uncached function) staying flat on the second call
    /// proves the whole function was skipped, not just its scan.
    #[test]
    fn reachability_repeat_query_skips_uncached_function_entirely() {
        let store = DocumentStore::new();
        for i in 0..5 {
            open(
                &store,
                uri(&format!("/Other/Noise{i}.php")),
                format!(
                    "<?php\nnamespace Other{i};\nclass N{i} {{ public function run() {{ return new \\App\\Owner(); }} }}\n"
                ),
            );
        }
        store.mark_index_ready();

        let target: Arc<str> = Arc::from("App\\Owner");
        store
            .fqn_reachable_files(&[target.clone()])
            .expect("index is ready");
        let passes_after_first = store.reachability_scan_passes();
        assert!(
            passes_after_first > 0,
            "cold query must run the uncached function at least once"
        );

        store
            .fqn_reachable_files(&[target])
            .expect("index is ready");
        assert_eq!(
            store.reachability_scan_passes(),
            passes_after_first,
            "identical repeat query must be answered entirely from the result cache"
        );
    }

    /// `batch_reference_candidate_files` can mix a bounded needle-less query
    /// (`Name::Class`, an FQN literal) and a raw-needle query (`__construct`,
    /// whose extra needle is the no-word-bound call token `->__construct`)
    /// in the SAME call — both answered by mir's `ClassMentionIndex`, which
    /// admits and scans both needle shapes in one pass (see
    /// `resolve_reachability_queries_uncached`'s doc comment). `UsesBoth.php`
    /// calls `$obj->__construct()` on an untyped parameter — it never
    /// textually mentions `Owner` at all, so it must surface only via the
    /// raw needle, never via the class query, proving the two needle shapes
    /// don't cross-pollute each other's results when batched together.
    #[test]
    fn batch_reference_candidate_files_mixes_raw_and_bounded_needles() {
        let store = DocumentStore::new();
        open(
            &store,
            uri("/App/Owner.php"),
            "<?php\nnamespace App;\nclass Owner { public function __construct() {} }\n"
                .to_string(),
        );
        open(
            &store,
            uri("/Other/UsesBoth.php"),
            "<?php\nnamespace Other;\nclass UsesBoth { public function run($obj) { $obj->__construct(); } }\n"
                .to_string(),
        );
        store.mark_index_ready();

        let symbols = vec![
            mir_analyzer::Name::Class(Arc::from("App\\Owner")),
            mir_analyzer::Name::Method {
                class: Arc::from("App\\Owner"),
                name: Arc::from("__construct"),
            },
        ];
        let scopes = store.batch_reference_candidate_files(&symbols);
        assert_eq!(scopes.len(), 2);
        let class_scope: Vec<&str> = scopes[0].iter().map(|f| f.as_ref()).collect();
        let method_scope: Vec<&str> = scopes[1].iter().map(|f| f.as_ref()).collect();
        assert!(
            !class_scope.iter().any(|f| f.contains("UsesBoth")),
            "the class query must not pick up a file that never mentions the FQN: {class_scope:?}"
        );
        assert!(
            method_scope.iter().any(|f| f.contains("UsesBoth")),
            "the raw `->__construct` needle must find the call site: {method_scope:?}"
        );
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

    fn salsa_index_names(store: &DocumentStore, url: &Uri) -> Vec<String> {
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
        let doc = store.get_doc_salsa(&u).unwrap();
        match &doc.program().stmts[0].kind {
            php_ast::StmtKind::Class(c) => {
                assert_eq!(c.name.and_then(|n| n.as_str()), Some("P"))
            }
            other => panic!("expected a class declaration, got {other:?}"),
        }
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
        let urls: Vec<Uri> = (0..8).map(|i| uri(&format!("/hl{i}.php"))).collect();
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
            url: &Uri,
            writers: usize,
            label: &str,
            call: impl Fn(&DocumentStore, &Uri) -> bool,
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
        let urls: Vec<Uri> = (0..8).map(|i| uri(&format!("/f{i}.php"))).collect();
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
        let handler_url = Uri::from_file_path(tmp.path().join("src/Service/Handler.php")).unwrap();
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

    /// Issue #243 repro: a PSR-0-autoloaded vendor class (PEAR-style, e.g.
    /// `Legacy_Service`) must lazy-load the same way PSR-4 classes do.
    #[test]
    fn psr0_lazy_load_suppresses_undefined_class_243_repro() {
        let tmp = tempfile::tempdir().unwrap();

        std::fs::create_dir_all(tmp.path().join("vendor/legacy/src/Legacy")).unwrap();
        std::fs::write(
            tmp.path().join("vendor/legacy/src/Legacy/Service.php"),
            "<?php\n\nclass Legacy_Service\n{\n    public function name(): string\n    {\n        return 'legacy';\n    }\n}\n",
        )
        .unwrap();

        std::fs::write(
            tmp.path().join("composer.json"),
            r#"{"autoload":{"psr-0":{"Legacy_":"vendor/legacy/src/"}}}"#,
        )
        .unwrap();

        let store = DocumentStore::new();
        store
            .psr4
            .store(Arc::new(crate::lang::autoload::Psr4Map::load(tmp.path())));

        let repro_url = Uri::from_file_path(tmp.path().join("repro.php")).unwrap();
        store.mirror_text(
            &repro_url,
            "<?php\n\n$service = new Legacy_Service();\necho $service->name();\n",
        );

        let issues = store.get_semantic_issues_salsa(&repro_url).unwrap();
        let undef: Vec<_> = issues
            .iter()
            .filter(|i| matches!(i.kind, mir_issues::IssueKind::UndefinedClass { .. }))
            .collect();
        assert!(
            undef.is_empty(),
            "PSR-0 lazy-loading must prevent UndefinedClass for Legacy_Service; got: {undef:?}"
        );
    }

    /// Regression (found while investigating issue #242, root cause is
    /// distinct from it — not fixed by the `is_index_ready` gate):
    /// `cached_analysis_if_fresh`'s staleness check is keyed on
    /// `decl_version`, which only bumps inside `cached_analysis_cancellable`
    /// when a file undergoes its *own* full analysis. Mirroring a brand new
    /// file into the store (`mirror_text`/`ingest_from_doc`, what a workspace
    /// scan or `didChangeWatchedFiles` CREATED event does) doesn't go through
    /// that path, so callers on that path must explicitly call
    /// `note_new_file_declarations` — exactly what `did_change_watched_files`
    /// now does — to invalidate consumers analyzed before the dependency
    /// existed.
    #[test]
    fn stale_cached_analysis_not_invalidated_by_new_dependency_file() {
        let store = DocumentStore::new();
        let consumer_uri = uri("/app.php");
        let dep_uri = uri("/Mage.php");

        // Consumer references Mage before Mage exists anywhere in the store —
        // correctly flagged, and this analysis gets cached.
        store.mirror_text(&consumer_uri, "<?php\n\nnew Mage();\n");
        let issues = store.get_semantic_issues_salsa(&consumer_uri).unwrap();
        assert!(
            issues
                .iter()
                .any(|i| matches!(i.kind, mir_issues::IssueKind::UndefinedClass { .. })),
            "sanity check: Mage must be reported missing before it exists"
        );

        // Mage now becomes known to the store — exactly what a workspace
        // scan or a `didChangeWatchedFiles` CREATED event does.
        store.mirror_text(&dep_uri, "<?php\nclass Mage {}\n");
        store.note_new_file_declarations(&dep_uri);

        // Consumer's diagnostics should now resolve cleanly.
        let issues = store.get_semantic_issues_salsa(&consumer_uri).unwrap();
        assert!(
            !issues
                .iter()
                .any(|i| matches!(i.kind, mir_issues::IssueKind::UndefinedClass { .. })),
            "Mage now exists, but consumer.php's cached analysis was never \
             invalidated: {issues:?}"
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

        let by_url_first: std::collections::HashMap<Uri, Arc<ParsedDoc>> =
            first.into_iter().collect();
        for (u, doc2) in second {
            let doc1 = by_url_first
                .get(&u)
                .expect("second scan returned a URL the first didn't");
            assert!(
                Arc::ptr_eq(doc1, &doc2),
                "{u:?} re-parsed across all_docs_for_scan calls — \
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
                "{u:?} should not have re-parsed because of an unrelated edit"
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

    /// Concurrent callers racing to analyze the SAME uncached file (e.g.
    /// `did_open`'s diagnostics trigger vs. a fast-following hover, which
    /// tower-lsp's default concurrency genuinely allows) must serialize onto
    /// one `FileAnalyzer::analyze` run and all observe its identical result
    /// — not each redo the full analysis independently.
    #[test]
    fn concurrent_callers_share_one_analysis_computation() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let store = Arc::new(DocumentStore::new());
        let u = uri("/inflight_test.php");
        open(
            &store,
            u.clone(),
            "<?php\nfunction f(): int { return 1; }".to_string(),
        );

        const N: usize = 8;
        let barrier = Arc::new(Barrier::new(N));
        let handles: Vec<_> = (0..N)
            .map(|_| {
                let store = Arc::clone(&store);
                let u = u.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    store.cached_analysis(&u)
                })
            })
            .collect();
        let results: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("no panic in cached_analysis"))
            .collect();

        assert_eq!(
            store.analysis_compute_count(),
            1,
            "{N} concurrent callers analyzing the same uncached file must compute once, not {N} times"
        );
        let first = results[0].clone().expect("analysis must succeed");
        for r in &results {
            let r = r.as_ref().expect("analysis must succeed");
            assert!(
                Arc::ptr_eq(&first, r),
                "every caller must observe the identical analysis Arc, not its own copy"
            );
        }
    }

    /// Manual perf diagnostic (`cargo test --release -- --ignored --nocapture
    /// diagnostic_coalescing_cpu_time_ms`): N threads race `cached_analysis`
    /// for the same uncached, moderately expensive file. `computations`
    /// (from `analysis_compute_count`) is the direct proof of the fix: it
    /// must stay at 1 regardless of N. `wall_ms` is secondary color — without
    /// the per-URI lock, N threads each independently running the full
    /// analyze pass genuinely in parallel can contend for the same cores/
    /// caches and degrade wall time as N grows past the core count; with the
    /// lock, added threads just wait on the one real computation.
    #[test]
    #[ignore]
    fn diagnostic_coalescing_cpu_time_ms() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        use std::time::Instant;

        let mut src = "<?php\nclass Big {\n".to_string();
        for m in 0..200 {
            src.push_str(&format!(
                "    public function m{m}(int $x): int {{ $y = $x * {m} + strlen((string)$x); return $y > 0 ? $y : -$y; }}\n"
            ));
        }
        src.push_str("}\n");

        for &nthreads in &[1usize, 4, 8, 16, 32] {
            let store = Arc::new(DocumentStore::new());
            let u = uri("/coalesce_bench.php");
            open(&store, u.clone(), src.clone());

            let barrier = Arc::new(Barrier::new(nthreads));
            let wall_start = Instant::now();
            let handles: Vec<_> = (0..nthreads)
                .map(|_| {
                    let store = Arc::clone(&store);
                    let u = u.clone();
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        let _ = store.cached_analysis(&u);
                    })
                })
                .collect();
            for h in handles {
                h.join().unwrap();
            }
            let wall_ms = wall_start.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "threads={nthreads:>3}  computations={:>3}  wall_ms={wall_ms:>8.3}",
                store.analysis_compute_count()
            );
        }
    }

    /// Manual perf diagnostic (`cargo test --release -- --ignored --nocapture
    /// diagnostic_batch_vs_per_symbol_scan_ms`), not a CI-run assertion:
    /// isolates the pure scan-count effect of `batch_reference_candidate_files`
    /// vs M independent `reference_candidate_files` calls, excluding the
    /// surrounding `code_lenses()`/`indexed_references` cost entirely — this
    /// measures scope narrowing alone. See `benches/code_lens_scaling.rs` for
    /// the end-to-end request-level number.
    #[test]
    #[ignore]
    fn diagnostic_batch_vs_per_symbol_scan_ms() {
        use std::time::Instant;
        let num_methods = 100usize;
        for &n_noise in &[500usize, 1500, 3000, 8000] {
            let store = DocumentStore::new();
            let owner = "App\\Service";
            let mut src = "<?php\nnamespace App;\nclass Service {\n".to_string();
            for m in 0..num_methods {
                src.push_str(&format!(
                    "    public static function alpha{m}(): void {{}}\n"
                ));
            }
            src.push_str("}\n");
            store.ingest(("file:///synth/Service.php").parse::<Uri>().unwrap(), &src);
            for i in 0..n_noise {
                let u = format!("file:///synth/N{i}.php").parse::<Uri>().unwrap();
                let t = format!(
                    "<?php\nnamespace Other{i};\nclass N{i} {{ public function run(){{}} }}\n"
                );
                store.ingest(u, &t);
            }
            store.mark_index_ready();

            let mut symbols = vec![mir_analyzer::Name::class(owner)];
            for m in 0..num_methods {
                symbols.push(mir_analyzer::Name::method(owner, &format!("alpha{m}")));
            }

            let t = Instant::now();
            for s in &symbols {
                std::hint::black_box(store.reference_candidate_files(s));
            }
            let per_symbol_ms = t.elapsed().as_secs_f64() * 1000.0;

            let t = Instant::now();
            std::hint::black_box(store.batch_reference_candidate_files(&symbols));
            let batch_ms = t.elapsed().as_secs_f64() * 1000.0;

            eprintln!(
                "n_noise={n_noise:>6}  symbols={:>4}  per_symbol_ms={per_symbol_ms:>10.3}  batch_ms={batch_ms:>10.3}  speedup={:>6.2}x",
                symbols.len(),
                per_symbol_ms / batch_ms
            );
        }
    }

    /// Manual perf diagnostic (`cargo test --release -- --ignored --nocapture
    /// diagnostic_priority_scan_avoided_cost_ms`): measures the cost of
    /// `handle_references`'s priority-partition owner-mention scan
    /// (`navigation.rs`) run over an already-narrowed candidate scope — the
    /// exact cost `method_scope_is_narrowed` now lets the handler skip
    /// entirely for private/protected/static methods. The scope size here
    /// models a protected method's subtype-closure narrowing (the case where
    /// a widely-subclassed base class makes the "narrowed" scope non-trivial).
    #[test]
    #[ignore]
    fn diagnostic_priority_scan_avoided_cost_ms() {
        use std::time::Instant;

        let store = DocumentStore::new();
        let owner_short = "Owner";
        for &n in &[10usize, 100, 1000, 3000] {
            let mut files: Vec<Arc<str>> = Vec::with_capacity(n);
            for i in 0..n {
                let u = format!("file:///synth/S{i}.php").parse::<Uri>().unwrap();
                let t = format!(
                    "<?php\nclass S{i} extends Owner {{\n\
                     \x20   public function run(): void {{\n\
                     \x20       parent::process();\n\
                     \x20   }}\n\
                     }}\n"
                );
                store.ingest(u.clone(), &t);
                files.push(Arc::<str>::from(u.as_str()));
            }

            let t0 = Instant::now();
            use rayon::prelude::*;
            let matched: Vec<Arc<str>> = files
                .par_iter()
                .filter(|f| {
                    (f.as_ref())
                        .parse::<Uri>()
                        .ok()
                        .and_then(|u| store.source_text(&u))
                        .is_some_and(|txt| {
                            crate::text::contains_ascii_case_insensitive(&txt, owner_short)
                        })
                })
                .cloned()
                .collect();
            let avoided_ms = t0.elapsed().as_secs_f64() * 1000.0;
            eprintln!(
                "narrowed_scope_files={n:>5}  avoided_scan_ms={avoided_ms:>9.4}  matched={}  (new code: 0.000ms — skipped entirely)",
                matched.len()
            );
        }
    }

    /// Manual perf diagnostic (`cargo test --release -- --ignored --nocapture
    /// diagnostic_laravel_scan_blocking_vs_spawn_blocking`): demonstrates the
    /// actual effect of moving `handle_references`'s Laravel string-key scan
    /// (`navigation.rs`) into `spawn_blocking`. On a single-worker-thread
    /// runtime, a CPU-bound scan with no `.await` points inside it hogs that
    /// one worker until it completes — a concurrently-spawned trivial task
    /// can't run until the scan yields. `spawn_blocking` moves the scan to a
    /// separate thread, freeing the worker immediately.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    #[ignore]
    async fn diagnostic_laravel_scan_blocking_vs_spawn_blocking() {
        use std::time::Instant;

        fn run_scan_inline(store: &DocumentStore, n: usize) -> usize {
            let mut count = 0;
            for i in 0..n {
                let u = format!("file:///synth/F{i}.php").parse::<Uri>().unwrap();
                if store
                    .source_text(&u)
                    .is_some_and(|t| t.contains("APP_NAME_0"))
                    && let Some(doc) = store.get_doc_salsa(&u)
                {
                    let _ = crate::laravel::find_call_sites(&doc, &["env"], "APP_NAME_0");
                    count += 1;
                }
            }
            count
        }

        let store = Arc::new(DocumentStore::new());
        let n = 30_000usize;
        for i in 0..n {
            let u = format!("file:///synth/F{i}.php").parse::<Uri>().unwrap();
            let t = format!("<?php\n$x = env('APP_NAME_{i}');\n");
            store.ingest(u, &t);
        }

        // Both scenarios spawn the scan AND a trivial task as separate
        // `tokio::spawn`ed tasks — the same relationship real tower-lsp
        // request handlers have (each dispatched request is its own spawned
        // task on the shared worker pool), unlike driving the scan inline in
        // this test function's own body (which `#[tokio::test]` runs via
        // `block_on` on the harness thread, not a pool worker, and so
        // wouldn't actually contend with `tokio::spawn`ed tasks at all).

        // OLD shape: scan task runs the work inline, no yield points inside.
        let spawn_time = Instant::now();
        let store_c = Arc::clone(&store);
        let scan_handle = tokio::spawn(async move { run_scan_inline(&store_c, n) });
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = tx.send(Instant::now());
        });
        let matched = scan_handle.await.unwrap();
        let scan_ms = spawn_time.elapsed().as_secs_f64() * 1000.0;
        let trivial_ran_at = rx.await.unwrap();
        let trivial_delay_ms = trivial_ran_at.duration_since(spawn_time).as_secs_f64() * 1000.0;
        eprintln!(
            "OLD (inline on the request's own task):  scan_ms={scan_ms:>8.3}  matched={matched:>4}  concurrent_trivial_task_delay_ms={trivial_delay_ms:>8.3}"
        );

        // NEW shape: scan task hands the work to spawn_blocking and awaits it.
        let spawn_time = Instant::now();
        let store_c = Arc::clone(&store);
        let scan_handle = tokio::spawn(async move {
            tokio::task::spawn_blocking(move || run_scan_inline(&store_c, n))
                .await
                .unwrap()
        });
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _ = tx.send(Instant::now());
        });
        let matched = scan_handle.await.unwrap();
        let scan_ms = spawn_time.elapsed().as_secs_f64() * 1000.0;
        let trivial_ran_at = rx.await.unwrap();
        let trivial_delay_ms = trivial_ran_at.duration_since(spawn_time).as_secs_f64() * 1000.0;
        eprintln!(
            "NEW (via spawn_blocking):                scan_ms={scan_ms:>8.3}  matched={matched:>4}  concurrent_trivial_task_delay_ms={trivial_delay_ms:>8.3}"
        );
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

    /// Same rebuild-on-race guarantee as `set_session_cache_dir`, for
    /// `set_user_stub_dirs`.
    #[test]
    fn set_user_stub_dirs_rebuilds_pinned_session() {
        let store = DocumentStore::new();
        let early = store.analysis_session(mir_analyzer::PhpVersion::LATEST);
        let dir = tempfile::tempdir().unwrap();
        store.set_user_stub_dirs(vec![dir.path().to_path_buf()]);
        let rebuilt = store.analysis_session(mir_analyzer::PhpVersion::LATEST);
        assert!(
            !Arc::ptr_eq(&early, &rebuilt),
            "the stub-dir-less early session must be dropped and rebuilt"
        );
    }

    /// A class defined only in a `stubDirs` directory must resolve — without
    /// `UndefinedClass` — from a *different* file than whichever one happened
    /// to trigger the session's lazy stub load, proving the stub is
    /// registered as a real, session-wide symbol rather than something
    /// scoped to the triggering file.
    #[test]
    fn user_stub_directory_class_resolves_across_files() {
        let stubs_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            stubs_dir.path().join("Container.php"),
            "<?php\nclass Container {\n    public function get(string $id): mixed { return null; }\n}\n",
        )
        .unwrap();

        let store = DocumentStore::new();
        store.set_user_stub_dirs(vec![stubs_dir.path().to_path_buf()]);

        // First file has nothing to do with the stub; only primes the session.
        let a = uri("/stub_cross_file_a.php");
        store.mirror_text(&a, "<?php\necho 1;\n");
        let _ = store.get_semantic_issues_salsa(&a);

        // Second, unrelated file references the stub-defined class.
        let b = uri("/stub_cross_file_b.php");
        store.mirror_text(&b, "<?php\n$c = new Container();\n$c->get('x');\n");
        let issues = store.get_semantic_issues_salsa(&b).unwrap();
        let undef: Vec<_> = issues
            .iter()
            .filter(|i| matches!(i.kind, mir_issues::IssueKind::UndefinedClass { .. }))
            .collect();
        assert!(
            undef.is_empty(),
            "Container must resolve from the stubDirs directory in a file that \
             didn't trigger the load itself; got: {undef:?}"
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
