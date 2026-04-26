use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use salsa::Setter;
use tower_lsp::lsp_types::{Diagnostic, SemanticToken, Url};

use crate::ast::ParsedDoc;
use crate::db::analysis::AnalysisHost;
use crate::db::input::{FileId, SourceFile, Workspace};
use crate::file_index::FileIndex;

/// Upper bound on `parsed_cache` entries. Matched to the `lru = 2048` on
/// `parsed_doc` in `src/db/parse.rs` so the secondary Arc retention can't
/// pin more ASTs alive than salsa's memo already bounds. Exceeding this
/// triggers probabilistic eviction (see [`DocumentStore::insert_parsed_cache`]).
const PARSED_CACHE_CAP: usize = 2048;

pub struct DocumentStore {
    /// Cached semantic tokens per document: (result_id, tokens).
    /// Used to compute incremental deltas for `textDocument/semanticTokens/full/delta`.
    token_cache: DashMap<Url, (String, Vec<SemanticToken>)>,

    // ── Salsa-input storage ────────────────────────────────────────────────
    // Phase E4: `DocumentStore` is now a pure salsa-input wrapper. Open-file
    // state (live text, version token, parse-diagnostics cache) lives on
    // `Backend` in its `open_files` map; the set of files tracked by salsa
    // is exactly `source_files.keys()`.
    /// Mutex — held briefly to clone the database for reads and to mutate
    /// it for writes. Per-thread salsa state (`zalsa_local`) is `!Sync`,
    /// which rules out `RwLock<AnalysisHost>`. Readers instead snapshot the
    /// db (cheap — storage is `Arc<Zalsa>`) and run queries on the clone
    /// with the lock released, giving real read/read parallelism. Writers
    /// during an in-flight read bump the shared revision; the reader raises
    /// `salsa::Cancelled` on its next query call and `snapshot_query` below
    /// retries with a fresh snapshot.
    host: Mutex<AnalysisHost>,
    /// `Url -> SourceFile` lookup. The `SourceFile` is a salsa-id handle; the
    /// underlying input lives in `host.db` for the lifetime of the database.
    source_files: DashMap<Url, SourceFile>,
    /// G2: lock-free mirror of each `SourceFile`'s last-set text. Lets
    /// `mirror_text` dedup repeated no-op updates (common during workspace
    /// scan and `did_open` for already-indexed files) without taking
    /// `host.lock()`. Updated inside the mutex whenever the salsa input is
    /// set, so it is always consistent with the salsa revision for the
    /// purposes of byte-equality comparison.
    text_cache: DashMap<Url, Arc<str>>,
    /// G3: cross-revision read-through cache for `parsed_doc`. Keyed on
    /// `Url`, stored value is `(text_arc, Arc<ParsedDoc>)` — the text Arc
    /// captured at parse time. On read, compare against `text_cache[uri]`
    /// via `Arc::ptr_eq`; a match guarantees the cached ParsedDoc matches
    /// the current salsa revision's text input, so the query can return
    /// without snapshotting the db or invoking salsa at all. A miss
    /// (different pointer, stale or absent entry) falls through to
    /// `snapshot_query`. Self-evicts on text change — no writer-side
    /// invalidation is required, which avoids the TOCTOU window where a
    /// concurrent reader could re-insert a stale entry after a writer's
    /// eviction.
    ///
    /// Size-bounded at [`PARSED_CACHE_CAP`] — see `insert_parsed_cache`.
    /// Without this bound, every workspace file read-through would pin
    /// its bumpalo arena alive regardless of salsa's `lru = 2048` on the
    /// `parsed_doc` memo.
    parsed_cache: DashMap<Url, (Arc<str>, Arc<ParsedDoc>)>,
    /// Monotonic allocator for `FileId`s (one per ever-seen URL).
    next_file_id: AtomicU32,
    /// Workspace salsa input. Tracks the full set of `SourceFile`s that
    /// participate in whole-program queries (`codebase`, `file_refs`).
    /// Re-synced from `source_files` on demand by `sync_workspace_files`.
    workspace: Workspace,
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentStore {
    pub fn new() -> Self {
        let host = AnalysisHost::new();
        let workspace = Workspace::new(
            host.db(),
            Arc::<[SourceFile]>::from(Vec::new()),
            mir_analyzer::PhpVersion::LATEST,
        );
        DocumentStore {
            token_cache: DashMap::new(),
            host: Mutex::new(host),
            source_files: DashMap::new(),
            text_cache: DashMap::new(),
            parsed_cache: DashMap::new(),
            next_file_id: AtomicU32::new(0),
            workspace,
        }
    }

    /// Mirror a file's current text into the salsa layer. Creates the
    /// `SourceFile` input on first sight, otherwise updates `text` on the
    /// existing input (bumping the salsa revision so downstream queries
    /// invalidate). Returns the `SourceFile` handle for this `uri`.
    ///
    /// B4a: called from every text-changing mutation site. Reads still come
    /// from the legacy `map` — this mirror is not yet observed by production
    /// code paths.
    pub fn mirror_text(&self, uri: &Url, text: &str) -> SourceFile {
        // G2 fast path: compare against the lock-free text cache. When the
        // new text byte-matches what we already mirrored, skip the host
        // mutex entirely. Common during workspace scan + `did_open` for
        // unchanged files, where most threads would otherwise serialise on
        // `host.lock()` just to confirm a no-op. Cache is only populated
        // after the matching `source_files` entry, so a cache hit implies
        // the handle exists.
        if let Some(cached) = self.text_cache.get(uri)
            && **cached == *text
            && let Some(sf) = self.source_files.get(uri)
        {
            return *sf;
        }

        let text_arc: Arc<str> = Arc::from(text);
        if let Some(existing) = self.source_files.get(uri) {
            let sf = *existing;
            drop(existing);
            // Slow path: another writer may have raced us; re-check inside
            // the mutex. Salsa's `set_text` unconditionally bumps the
            // revision, so every spurious setter invalidates every
            // downstream query.
            let mut host = self.host.lock().unwrap();
            let current: Arc<str> = sf.text(host.db());
            if *current == *text_arc {
                drop(host);
                self.text_cache.insert(uri.clone(), current);
                return sf;
            }
            sf.set_text(host.db_mut()).to(text_arc.clone());
            // Phase K2: any text change invalidates a previously-seeded
            // cached slice. Clearing it forces the fresh-parse branch of
            // `file_definitions` on the next query, which is correct —
            // the cached slice no longer matches the new text.
            sf.set_cached_slice(host.db_mut()).to(None);
            drop(host);
            self.text_cache.insert(uri.clone(), text_arc);
            sf
        } else {
            let id = FileId(self.next_file_id.fetch_add(1, Ordering::Relaxed));
            let uri_arc: Arc<str> = Arc::from(uri.as_str());
            let sf = {
                let host = self.host.lock().unwrap();
                SourceFile::new(host.db(), id, uri_arc, text_arc.clone(), None)
            };
            self.source_files.insert(uri.clone(), sf);
            self.text_cache.insert(uri.clone(), text_arc);
            sf
        }
    }

    /// Return the salsa `SourceFile` handle for a URL, if one exists.
    pub fn source_file(&self, uri: &Url) -> Option<SourceFile> {
        self.source_files.get(uri).map(|e| *e)
    }

    /// Phase K2: pre-seed a `StubSlice` loaded from the on-disk cache
    /// onto the `SourceFile` input for `uri`. The next `file_definitions`
    /// call for that file returns the cached slice directly, skipping
    /// parse + `DefinitionCollector`.
    ///
    /// Must be called **before** any `file_definitions(db, sf)` call for
    /// this file — otherwise salsa has already memoized the fresh-parse
    /// result and setting `cached_slice` now would only bump the revision
    /// without actually using the cache. In practice the workspace-scan
    /// path seeds immediately after `mirror_text` and before any query
    /// runs.
    ///
    /// Returns `false` when `uri` was not mirrored (caller should mirror
    /// first); returns `true` on success.
    pub fn seed_cached_slice(
        &self,
        uri: &Url,
        slice: Arc<mir_codebase::storage::StubSlice>,
    ) -> bool {
        let Some(sf) = self.source_files.get(uri).map(|e| *e) else {
            return false;
        };
        let mut host = self.host.lock().unwrap();
        sf.set_cached_slice(host.db_mut()).to(Some(slice));
        true
    }

    /// Run `f` with a borrow of the `AnalysisHost`. Used by tests and by the
    /// upcoming `*_salsa` accessors to query the salsa layer.
    pub fn with_host<R>(&self, f: impl FnOnce(&AnalysisHost) -> R) -> R {
        let host = self.host.lock().unwrap();
        f(&host)
    }

    /// Phase E1: take a brief lock, clone the salsa database, release the
    /// lock. Queries then run on the cloned `RootDatabase` without blocking
    /// writers or other readers. Salsa's `Storage<Self>` is reference-counted
    /// (`Arc<Zalsa>`), so the clone is cheap — it shares memoized data and
    /// the cancellation flag with the host's db.
    fn snapshot_db(&self) -> crate::db::analysis::RootDatabase {
        let host = self.host.lock().unwrap();
        host.db().clone()
    }

    /// Run a query on a fresh snapshot, catching `salsa::Cancelled` (raised
    /// when a concurrent writer advances the revision) and retrying with a
    /// new snapshot. Writers hold the mutex only long enough to bump input
    /// values, so a handful of retries is more than enough in practice; we
    /// cap at 8 to avoid pathological livelock under sustained write pressure.
    fn snapshot_query<R>(&self, f: impl Fn(&crate::db::analysis::RootDatabase) -> R + Clone) -> R {
        use std::panic::AssertUnwindSafe;
        for _ in 0..8 {
            let db = self.snapshot_db();
            let f = f.clone();
            match salsa::Cancelled::catch(AssertUnwindSafe(move || f(&db))) {
                Ok(r) => return r,
                Err(_) => continue,
            }
        }
        // Last-resort attempt: take the mutex for the whole query so no
        // writer can race us. Much slower, but guaranteed to make progress.
        let host = self.host.lock().unwrap();
        f(host.db())
    }

    /// Evict the semantic-tokens cache for `uri`. Called by Backend when a
    /// file is closed; diff-based tokens computed against the old revision
    /// are no longer meaningful.
    pub fn evict_token_cache(&self, uri: &Url) {
        self.token_cache.remove(uri);
    }

    /// Register a file in the salsa layer without marking it open.
    ///
    /// Salsa's `parsed_doc` query parses lazily on first read; diagnostics
    /// are populated by `did_open` when the editor actually opens the file.
    pub fn index(&self, uri: Url, text: &str) {
        self.mirror_text(&uri, text);
    }

    /// Index a file using an already-parsed `ParsedDoc`, avoiding a second parse.
    ///
    /// Prefer this over [`index`] when the caller already has a `ParsedDoc` (e.g.
    /// after running `DefinitionCollector` during workspace scan).
    ///
    /// `_diagnostics` is accepted for call-site compatibility; parse
    /// diagnostics for background-indexed files are never consulted
    /// (callers gate on `get_doc_salsa` returning `Some`).
    pub fn index_from_doc(&self, uri: Url, doc: &ParsedDoc, _diagnostics: Vec<Diagnostic>) {
        self.mirror_text(&uri, doc.source());
    }

    pub fn remove(&self, uri: &Url) {
        self.token_cache.remove(uri);
        // Also drop the Url→SourceFile mapping so the file stops contributing
        // to the workspace codebase query. Salsa inputs themselves remain
        // alive (salsa doesn't expose input removal in 0.26), but they're
        // orphaned — no query keys them anymore, and re-opening the file
        // allocates a fresh SourceFile with a new FileId. The ~40 bytes per
        // orphan is acceptable; revisit if workspace-churn profiling hurts.
        self.source_files.remove(uri);
        self.text_cache.remove(uri);
        self.parsed_cache.remove(uri);
    }

    // ── B4b salsa-backed accessors ─────────────────────────────────────────
    //
    // These are additive and not yet called from production code. They go
    // through the salsa layer — reads run the memoized `parsed_doc` /
    // `file_index` / `method_returns` queries, parsing only on first access
    // per revision. B4c will migrate feature modules to call these instead of
    // the legacy `get_doc` / `get_index`.

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
        let sf = self.source_file(uri)?;
        Some(self.snapshot_query(move |db| crate::db::index::file_index(db, sf).0.clone()))
    }

    /// G3: shared implementation for `get_doc_salsa`.
    /// Tries the `parsed_cache` (lock-free) first; validates via
    /// `Arc::ptr_eq` against the G2 `text_cache` so a concurrent writer
    /// that has already committed a new text input cannot be masked by a
    /// stale cache entry. On miss, captures the text Arc and ParsedDoc
    /// together inside a single `snapshot_query`, then publishes both.
    fn get_parsed_cached(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        if let Some(current_text) = self.text_cache.get(uri)
            && let Some(entry) = self.parsed_cache.get(uri)
            && Arc::ptr_eq(&*current_text, &entry.0)
        {
            return Some(entry.1.clone());
        }

        let sf = self.source_file(uri)?;
        let (text, doc) = self.snapshot_query(move |db| {
            let text = sf.text(db);
            let doc = crate::db::parse::parsed_doc(db, sf).0.clone();
            (text, doc)
        });
        self.insert_parsed_cache(uri.clone(), text, doc.clone());
        Some(doc)
    }

    /// Publish a fresh `ParsedDoc` into `parsed_cache`, shedding roughly
    /// half of the cache first if it has grown past [`PARSED_CACHE_CAP`].
    ///
    /// Eviction is probabilistic (DashMap iteration order is arbitrary),
    /// not LRU. That's fine — salsa's own `parsed_doc` memo uses
    /// `lru = 2048` on hotness-aware storage, so a cache-miss here is
    /// cheap: the next read goes through `snapshot_query` and
    /// `parsed_doc`, which still short-circuits on the salsa memo.
    /// What we're bounding here is the *secondary* Arc retention that
    /// would otherwise pin every workspace file's bumpalo arena alive
    /// regardless of salsa's eviction decisions.
    fn insert_parsed_cache(&self, uri: Url, text: Arc<str>, doc: Arc<ParsedDoc>) {
        if self.parsed_cache.len() >= PARSED_CACHE_CAP {
            let drop_target = self.parsed_cache.len() / 2;
            let mut dropped = 0usize;
            self.parsed_cache.retain(|_, _| {
                if dropped < drop_target {
                    dropped += 1;
                    false
                } else {
                    true
                }
            });
        }
        self.parsed_cache.insert(uri, (text, doc));
    }

    /// Refresh `workspace.files` to mirror the current `source_files` set.
    ///
    /// Called by `get_codebase_salsa`. Skips the setter when the file list
    /// hasn't changed — salsa's `set_field` unconditionally bumps revision,
    /// which would invalidate every downstream query (codebase, file_refs).
    /// Dedup is essential for memoization across LSP requests.
    pub fn sync_workspace_files(&self) {
        let mut files: Vec<SourceFile> = self.source_files.iter().map(|e| *e.value()).collect();
        files.sort_by_key(|sf| self.with_host(|host| sf.id(host.db()).0));
        let mut host = self.host.lock().unwrap();
        let current = self.workspace.files(host.db());
        if current.len() == files.len() && current.iter().zip(files.iter()).all(|(a, b)| a == b) {
            return;
        }
        let arc: Arc<[SourceFile]> = Arc::from(files);
        self.workspace.set_files(host.db_mut()).to(arc);
    }

    /// Update the PHP version tracked by the workspace. Salsa will invalidate
    /// all `semantic_issues` queries so diagnostics are re-evaluated.
    /// Skips the setter when the version hasn't changed to avoid spurious
    /// query invalidation.
    pub fn set_php_version(&self, version: mir_analyzer::PhpVersion) {
        let mut host = self.host.lock().unwrap();
        if self.workspace.php_version(host.db()) == version {
            return;
        }
        self.workspace.set_php_version(host.db_mut()).to(version);
    }

    /// Salsa-backed finalized Codebase. Aggregates every known file's
    /// `StubSlice` via `codebase_from_parts`, memoized by salsa.
    ///
    /// Phase C step 3: this runs in parallel with Backend's imperative
    /// `Arc<Codebase>`. Comparison tests validate parity; readers migrate in
    /// a follow-up.
    pub fn get_codebase_salsa(&self) -> Arc<mir_codebase::Codebase> {
        self.sync_workspace_files();
        let ws = self.workspace;
        self.snapshot_query(move |db| crate::db::codebase::codebase(db, ws).0.clone())
    }

    /// Salsa-backed reference lookup — drop-in replacement for
    /// `Codebase::get_reference_locations`. First call per `key` runs
    /// `file_refs` over every workspace file; subsequent calls hit the
    /// `symbol_refs` memo.
    pub fn get_symbol_refs_salsa(&self, key: &str) -> Vec<(Arc<str>, u32, u32)> {
        self.sync_workspace_files();
        let ws = self.workspace;
        let key = key.to_string();
        self.snapshot_query(move |db| {
            warm_file_refs_parallel(db, ws);
            crate::db::refs::symbol_refs(db, ws, key.clone())
                .0
                .as_ref()
                .clone()
        })
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
        let ws = self.workspace;
        self.snapshot_query(move |db| {
            crate::db::workspace_index::workspace_index(db, ws)
                .0
                .clone()
        })
    }

    /// Phase L: force `file_refs` to run for every workspace file so that
    /// subsequent `textDocument/references` / `prepare_rename` / call-hierarchy
    /// lookups hit the memo instead of paying first-call latency.
    ///
    /// Uses parallel warming (`warm_file_refs_parallel`) so all `file_refs`
    /// complete concurrently; `symbol_refs` then only aggregates memos.
    pub fn warm_reference_index(&self) {
        self.sync_workspace_files();
        let ws = self.workspace;
        let _ = self.snapshot_query(move |db| {
            warm_file_refs_parallel(db, ws);
            crate::db::refs::symbol_refs(db, ws, String::from("__phplsp_warmup__"))
                .0
                .clone()
        });
    }

    /// Phase K2b: run `file_definitions` for `uri` and return the
    /// resulting `StubSlice`. Used by the workspace-scan write path to
    /// persist slices to disk after a cache miss.
    pub fn slice_for(&self, uri: &Url) -> Option<Arc<mir_codebase::storage::StubSlice>> {
        let sf = self.source_file(uri)?;
        Some(
            self.snapshot_query(move |db| {
                crate::db::definitions::file_definitions(db, sf).0.clone()
            }),
        )
    }

    /// Salsa-backed per-file method-return-type map.
    pub fn get_method_returns_salsa(&self, uri: &Url) -> Option<Arc<crate::ast::MethodReturnsMap>> {
        let sf = self.source_file(uri)?;
        Some(
            self.snapshot_query(move |db| {
                crate::db::method_returns::method_returns(db, sf).0.clone()
            }),
        )
    }

    /// Cache the semantic tokens computed for a delta response.
    /// `result_id` is an opaque string (a hash of the token data) returned to the client.
    pub fn store_token_cache(&self, uri: &Url, result_id: String, tokens: Vec<SemanticToken>) {
        self.token_cache.insert(uri.clone(), (result_id, tokens));
    }

    /// Return the cached tokens if `result_id` matches the stored one.
    pub fn get_token_cache(&self, uri: &Url, result_id: &str) -> Option<Vec<SemanticToken>> {
        self.token_cache
            .get(uri)
            .filter(|e| e.0.as_str() == result_id)
            .map(|e| e.1.clone())
    }

    /// Phase I: salsa-memoized raw semantic issues for a file. Callers apply
    /// their own `DiagnosticsConfig` filter via
    /// [`crate::semantic_diagnostics::issues_to_diagnostics`] — keeping the
    /// filter outside the query preserves memoization across config toggles.
    pub fn get_semantic_issues_salsa(&self, uri: &Url) -> Option<Arc<[mir_issues::Issue]>> {
        let sf = self.source_file(uri)?;
        self.sync_workspace_files();
        let ws = self.workspace;
        Some(
            self.snapshot_query(move |db| {
                crate::db::semantic::semantic_issues(db, ws, sf).0.clone()
            }),
        )
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

    /// `(primary, doc)` first, then every other open file's parsed doc.
    /// The `open_urls` slice should include `uri` — this helper filters it out.
    pub fn doc_with_others(
        &self,
        uri: &Url,
        doc: Arc<ParsedDoc>,
        open_urls: &[Url],
    ) -> Vec<(Url, Arc<ParsedDoc>)> {
        let mut result = vec![(uri.clone(), doc)];
        result.extend(self.other_docs(uri, open_urls));
        result
    }

    /// Parsed docs for every entry in `open_urls` except `uri`.
    pub fn other_docs(&self, uri: &Url, open_urls: &[Url]) -> Vec<(Url, Arc<ParsedDoc>)> {
        open_urls
            .iter()
            .filter(|u| *u != uri)
            .filter_map(|u| self.get_doc_salsa(u).map(|d| (u.clone(), d)))
            .collect()
    }

    /// Batched salsa fetch for every entry in `open_urls` except `uri`:
    /// returns each `(uri, ParsedDoc, MethodReturnsMap)` triple in a single
    /// `snapshot_query` so cancellation retries don't run N times.
    pub fn other_docs_with_returns(
        &self,
        uri: &Url,
        open_urls: &[Url],
    ) -> Vec<(Url, Arc<ParsedDoc>, Arc<crate::ast::MethodReturnsMap>)> {
        let source_files: Vec<(Url, crate::db::input::SourceFile)> = open_urls
            .iter()
            .filter(|u| *u != uri)
            .filter_map(|u| self.source_file(u).map(|sf| (u.clone(), sf)))
            .collect();
        if source_files.is_empty() {
            return Vec::new();
        }
        self.snapshot_query(move |db| {
            source_files
                .iter()
                .map(|(u, sf)| {
                    let doc = crate::db::parse::parsed_doc(db, *sf).0.clone();
                    let mr = crate::db::method_returns::method_returns(db, *sf).0.clone();
                    (u.clone(), doc, mr)
                })
                .collect()
        })
    }

    /// Compact symbol index for every mirrored file.
    pub fn all_indexes(&self) -> Vec<(Url, Arc<FileIndex>)> {
        let urls: Vec<Url> = self.source_files.iter().map(|e| e.key().clone()).collect();
        urls.into_iter()
            .filter_map(|u| self.get_index_salsa(&u).map(|idx| (u, idx)))
            .collect()
    }

    /// Same as `all_indexes` but excludes `uri`.
    pub fn other_indexes(&self, uri: &Url) -> Vec<(Url, Arc<FileIndex>)> {
        let urls: Vec<Url> = self
            .source_files
            .iter()
            .filter(|e| e.key() != uri)
            .map(|e| e.key().clone())
            .collect();
        urls.into_iter()
            .filter_map(|u| self.get_index_salsa(&u).map(|idx| (u, idx)))
            .collect()
    }

    /// Parsed documents for every mirrored file (open or background-indexed).
    /// Suitable for full-scan operations: find-references, rename,
    /// call_hierarchy, code_lens.
    pub fn all_docs_for_scan(&self) -> Vec<(Url, Arc<ParsedDoc>)> {
        let urls: Vec<Url> = self.source_files.iter().map(|e| e.key().clone()).collect();
        urls.into_iter()
            .filter_map(|u| self.get_doc_salsa(&u).map(|d| (u, d)))
            .collect()
    }
}

/// Run `file_refs` for every workspace file in parallel.
///
/// `db` clones are cheap (they share the same `Arc<Zalsa>` memo store), so
/// results computed on any clone are immediately visible to all others at the
/// same revision.  After this returns, the sequential loop inside `symbol_refs`
/// only does cheap memo lookups instead of running `StatementsAnalyzer` on
/// every file one-by-one.
///
/// Per-task `salsa::Cancelled` is caught and swallowed.  If the revision was
/// bumped, the main thread's next salsa call inside `symbol_refs` will raise
/// `Cancelled` too and `snapshot_query` retries the whole operation from
/// scratch.  If the revision was not bumped, any file whose task was cancelled
/// before completion simply has no memo entry and `symbol_refs`'s sequential
/// loop recomputes it.
fn warm_file_refs_parallel(
    db: &crate::db::analysis::RootDatabase,
    ws: crate::db::input::Workspace,
) {
    let files: Vec<_> = ws.files(db).iter().copied().collect();
    // Pre-clone one snapshot per file before entering the scope.
    // RootDatabase: Send (ZalsaLocal owns its RefCell; Arc<Zalsa> is Sync),
    // but RootDatabase: !Sync, so we must avoid sharing &RootDatabase across
    // threads.  Collecting owned clones first and moving each into its task
    // requires only Send, not Sync.
    let snaps: Vec<crate::db::analysis::RootDatabase> = files.iter().map(|_| db.clone()).collect();
    rayon::scope(move |s| {
        for (sf, snap) in files.into_iter().zip(snaps) {
            s.spawn(move |_| {
                let _ = salsa::Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                    crate::db::refs::file_refs(&snap, ws, sf);
                }));
            });
        }
    });
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

    #[test]
    fn salsa_codebase_matches_imperative_codebase() {
        // Parity check for Phase C step 3: the salsa-built codebase should
        // contain exactly the same class/interface/function FQNs as one
        // built imperatively via DefinitionCollector against a fresh
        // mir_codebase::Codebase.
        let store = DocumentStore::new();
        let sources = [
            (
                "/a.php",
                "<?php\nnamespace A;\nclass Foo {}\ninterface IX {}",
            ),
            (
                "/b.php",
                "<?php\nnamespace B;\nfunction bar(): int { return 1; }",
            ),
            ("/c.php", "<?php\nnamespace C;\nenum Color { case Red; }"),
        ];
        for (p, src) in &sources {
            open(&store, uri(p), src.to_string());
        }

        let salsa_cb = store.get_codebase_salsa();

        let imperative_cb = mir_codebase::Codebase::new();
        for (p, src) in &sources {
            let (doc, _) = crate::diagnostics::parse_document(src);
            let file: Arc<str> = Arc::from(uri(p).as_str());
            let map = php_rs_parser::source_map::SourceMap::new(src);
            let c =
                mir_analyzer::collector::DefinitionCollector::new(&imperative_cb, file, src, &map);
            let _ = c.collect(doc.program());
        }
        imperative_cb.finalize();

        for fqn in ["A\\Foo", "A\\IX", "C\\Color"] {
            assert_eq!(
                salsa_cb.type_exists(fqn),
                imperative_cb.type_exists(fqn),
                "parity mismatch on type {fqn}"
            );
            assert!(salsa_cb.type_exists(fqn), "{fqn} missing from salsa cb");
        }
        assert_eq!(
            salsa_cb.function_exists("B\\bar"),
            imperative_cb.function_exists("B\\bar"),
        );
        assert!(salsa_cb.function_exists("B\\bar"));
    }

    #[test]
    fn index_registers_file_in_salsa() {
        let store = DocumentStore::new();
        store.index(uri("/lib.php"), "<?php\nfunction lib_fn() {}");
        let idx = store.get_index_salsa(&uri("/lib.php")).unwrap();
        assert_eq!(idx.functions.len(), 1);
        assert_eq!(idx.functions[0].name, "lib_fn");
    }

    #[test]
    fn remove_drops_salsa_input() {
        let store = DocumentStore::new();
        store.index(uri("/lib.php"), "<?php");
        store.remove(&uri("/lib.php"));
        assert!(store.get_index_salsa(&uri("/lib.php")).is_none());
    }

    #[test]
    fn all_indexes_includes_every_mirrored_file() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nfunction a() {}".to_string());
        store.index(uri("/b.php"), "<?php\nfunction b() {}");
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
        store.store_token_cache(&u, "id1".to_string(), vec![]);
        assert!(store.get_token_cache(&u, "id1").is_some());
        store.evict_token_cache(&u);
        assert!(store.get_token_cache(&u, "id1").is_none());
    }

    #[test]
    fn index_populates_file_index_with_symbols() {
        let store = DocumentStore::new();
        store.index(uri("/a.php"), "<?php\nfunction hello() {}");
        let idx = store.get_index_salsa(&uri("/a.php")).unwrap();
        assert_eq!(idx.functions.len(), 1);
        assert_eq!(idx.functions[0].name, "hello");
    }

    #[test]
    fn open_populates_file_index_with_symbols() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nclass Foo {}".to_string());
        let idx = store.get_index_salsa(&uri("/a.php")).unwrap();
        assert_eq!(idx.classes.len(), 1);
        assert_eq!(idx.classes[0].name, "Foo");
    }

    // ── Mirror invariants ────────────────────────────────────────────────
    //
    // Every mutation path that changes file text must keep the salsa layer
    // consistent. These tests walk a set-edit-reopen cycle and assert that
    // the salsa-derived `FileIndex` reflects the latest text at each step.

    fn names_of(idx: &FileIndex) -> Vec<String> {
        let mut out: Vec<String> = idx.classes.iter().map(|c| c.name.clone()).collect();
        out.extend(idx.functions.iter().map(|f| f.name.clone()));
        out.sort();
        out
    }

    fn salsa_index_names(store: &DocumentStore, url: &Url) -> Vec<String> {
        let sf = store.source_file(url).expect("mirror recorded SourceFile");
        store.with_host(|host| {
            let arc = crate::db::index::file_index(host.db(), sf);
            names_of(arc.get())
        })
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
    fn mirror_tracks_index_and_index_from_doc() {
        let store = DocumentStore::new();

        // Background `index(url, text)` path.
        let u1 = uri("/bg1.php");
        store.index(u1.clone(), "<?php\nclass Bg1 {}");
        assert_eq!(salsa_index_names(&store, &u1), vec!["Bg1".to_string()]);

        // `index_from_doc(url, &doc, diags)` path (workspace-scan Phase 2).
        let u2 = uri("/bg2.php");
        let (doc, diags) =
            crate::diagnostics::parse_document("<?php\nclass Bg2 {}\nfunction f() {}");
        store.index_from_doc(u2.clone(), &doc, diags);
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
    /// Phase K2 end-to-end: seed a cached slice through `DocumentStore`,
    /// confirm the workspace codebase sees the cached fact, then edit the
    /// text and confirm the cache is cleared (codebase now reflects the
    /// re-parsed text). Exercises `seed_cached_slice` + `mirror_text`'s
    /// `set_cached_slice(None)` invalidation together.
    #[test]
    fn seed_cached_slice_then_edit_invalidates() {
        let store = DocumentStore::new();
        let u = uri("/seed_e2e.php");

        // Mirror the initial text — classes: "Real".
        store.mirror_text(&u, "<?php\nclass Real {}");

        // Build a cached slice claiming classes: "Seeded", for the same URI.
        let seeded = {
            let src = "<?php\nclass Seeded {}";
            let source_map = php_rs_parser::source_map::SourceMap::new(src);
            let (doc, _) = crate::diagnostics::parse_document(src);
            let collector = mir_analyzer::collector::DefinitionCollector::new_for_slice(
                Arc::<str>::from(u.as_str()),
                src,
                &source_map,
            );
            let (s, _) = collector.collect_slice(doc.program());
            Arc::new(s)
        };
        assert!(store.seed_cached_slice(&u, seeded));

        // Codebase should contain the seeded class, not the real one.
        let cb = store.get_codebase_salsa();
        assert!(cb.type_exists("Seeded"));
        assert!(!cb.type_exists("Real"));

        // Edit: mirror_text flips the text and also clears cached_slice.
        store.mirror_text(&u, "<?php\nclass Edited {}");
        let cb = store.get_codebase_salsa();
        assert!(
            cb.type_exists("Edited"),
            "after edit, codebase must reflect fresh parse"
        );
        assert!(
            !cb.type_exists("Seeded"),
            "mirror_text must clear cached_slice so stale data is gone"
        );
    }

    /// Seeding for a URL that was never mirrored is a no-op (returns `false`)
    /// — avoids silently allocating SourceFiles outside `mirror_text`'s control.
    #[test]
    fn seed_cached_slice_noops_for_unknown_uri() {
        let store = DocumentStore::new();
        let u = uri("/never_mirrored.php");
        let slice = Arc::new(mir_codebase::storage::StubSlice::default());
        assert!(!store.seed_cached_slice(&u, slice));
    }

    /// entries survive.
    #[test]
    fn parsed_cache_stays_bounded_under_many_inserts() {
        let store = DocumentStore::new();
        let overflow = PARSED_CACHE_CAP + 100;
        for i in 0..overflow {
            let u = uri(&format!("/cap/file{i}.php"));
            store.index(u.clone(), "<?php\nclass A {}");
            // Force a parsed_cache insert via get_doc_salsa.
            let _ = store.get_doc_salsa(&u);
        }
        assert!(
            store.parsed_cache.len() <= PARSED_CACHE_CAP,
            "parsed_cache grew to {} entries (cap {})",
            store.parsed_cache.len(),
            PARSED_CACHE_CAP
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
        store.index(u.clone(), "<?php\nclass P {}");
        assert!(store.get_doc_salsa(&u).is_some());
    }

    #[test]
    fn get_salsa_accessors_return_none_for_unknown_uri() {
        let store = DocumentStore::new();
        let u = uri("/never-seen.php");
        assert!(store.get_doc_salsa(&u).is_none());
        assert!(store.get_index_salsa(&u).is_none());
        assert!(store.get_method_returns_salsa(&u).is_none());
    }

    /// Phase E1: concurrent readers and writers must not deadlock, panic, or
    /// return stale data. Writers briefly bump inputs while readers are
    /// running on cloned snapshots; any `salsa::Cancelled` raised on the
    /// reader side must be caught and retried by `snapshot_query`.
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
                    let _ = store.get_codebase_salsa();
                    let _ = store.get_symbol_refs_salsa("C0");
                }
            }));
        }

        for h in handles {
            h.join().expect("no panic under concurrent read/write");
        }
    }

    /// Phase L: warm-up must not error and must pre-populate the `file_refs`
    /// memo. We can't cheaply observe salsa memo state from outside, so we
    /// instead call `warm_reference_index` and then verify that a real
    /// reference lookup returns the expected result — the warm-up running
    /// without panic across a realistic two-file workspace is the load-bearing
    /// guarantee.
    #[test]
    fn warm_reference_index_does_not_panic_and_keeps_lookups_correct() {
        let store = DocumentStore::new();
        open(
            &store,
            uri("/wa.php"),
            "<?php\nfunction a() { b(); }".to_string(),
        );
        open(
            &store,
            uri("/wb.php"),
            "<?php\nfunction b() {}\na();".to_string(),
        );
        store.warm_reference_index();
        let refs_to_a = store.get_symbol_refs_salsa("a");
        assert!(
            refs_to_a.iter().any(|(uri, _, _)| uri.contains("wb.php")),
            "reference to a() from /wb.php should be discoverable after warm-up, got {refs_to_a:?}"
        );
    }
}
