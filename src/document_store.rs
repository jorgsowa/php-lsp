use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use salsa::Setter;
use tower_lsp::lsp_types::{Diagnostic, SemanticToken, Url};

use crate::ast::ParsedDoc;
use crate::db::analysis::AnalysisHost;
use crate::db::input::{FileId, SourceFile, Workspace};
use crate::file_index::FileIndex;

/// Default limit used in tests so eviction can be exercised without many files.
#[cfg(test)]
pub(crate) const DEFAULT_MAX_INDEXED: usize = 3;
/// Default maximum number of indexed-only (not open in editor) files kept in memory.
#[cfg(not(test))]
pub(crate) const DEFAULT_MAX_INDEXED: usize = 1_000;

struct Document {
    /// `Some` when the file is open in the editor; `None` for workspace-indexed files.
    /// This remains the source of truth for "is this file open" — used by LRU
    /// eviction and by the salsa `get_doc_salsa` gating.
    text: Option<String>,
    /// Parse-level diagnostics. Later phases will derive these from the salsa
    /// `parsed_doc` query.
    diagnostics: Vec<Diagnostic>,
    /// Semantic diagnostics computed by `did_open`/`did_change`.
    /// Stored separately so callers like `code_action` can read them without
    /// rerunning the full codebase rebuild that produces them.
    sem_diagnostics: Vec<Diagnostic>,
    /// Incremented on every `set_text` call; used to discard stale async parse results.
    text_version: u64,
}

pub struct DocumentStore {
    map: DashMap<Url, Document>,
    /// Insertion-order queue of indexed-only URIs for LRU eviction.
    indexed_order: Mutex<VecDeque<Url>>,
    /// Cached semantic tokens per document: (result_id, tokens).
    /// Used to compute incremental deltas for `textDocument/semanticTokens/full/delta`.
    token_cache: DashMap<Url, (String, Vec<SemanticToken>)>,
    /// Maximum number of indexed-only files to keep in memory.
    max_indexed: AtomicUsize,

    // ── Phase B4a salsa mirror ──────────────────────────────────────────────
    // The salsa layer runs in parallel to the legacy `map` during migration.
    // Every mutation that changes a file's text also updates the salsa input;
    // reads are unchanged for now. Once all feature modules are migrated to
    // `*_salsa` accessors (B4c), the legacy fields above will be removed.
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
        let workspace = Workspace::new(host.db(), Arc::<[SourceFile]>::from(Vec::new()));
        DocumentStore {
            map: DashMap::new(),
            indexed_order: Mutex::new(VecDeque::new()),
            token_cache: DashMap::new(),
            max_indexed: AtomicUsize::new(DEFAULT_MAX_INDEXED),
            host: Mutex::new(host),
            source_files: DashMap::new(),
            text_cache: DashMap::new(),
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
    fn mirror_text(&self, uri: &Url, text: &str) -> SourceFile {
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
            drop(host);
            self.text_cache.insert(uri.clone(), text_arc);
            sf
        } else {
            let id = FileId(self.next_file_id.fetch_add(1, Ordering::Relaxed));
            let uri_arc: Arc<str> = Arc::from(uri.as_str());
            let sf = {
                let host = self.host.lock().unwrap();
                SourceFile::new(host.db(), id, uri_arc, text_arc.clone())
            };
            self.source_files.insert(uri.clone(), sf);
            self.text_cache.insert(uri.clone(), text_arc);
            sf
        }
    }

    /// Return the salsa `SourceFile` handle for a URL, if one exists.
    #[allow(dead_code)]
    pub fn source_file(&self, uri: &Url) -> Option<SourceFile> {
        self.source_files.get(uri).map(|e| *e)
    }

    /// Run `f` with a borrow of the `AnalysisHost`. Used by tests and by the
    /// upcoming `*_salsa` accessors to query the salsa layer.
    #[allow(dead_code)]
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

    /// Update the maximum number of indexed-only files kept in memory.
    /// Excess entries are evicted immediately.
    pub fn set_max_indexed(&self, limit: usize) {
        self.max_indexed.store(limit, Ordering::Relaxed);
        let mut order = self.indexed_order.lock().unwrap();
        let need_to_evict = order.len().saturating_sub(limit);
        let mut evicted = 0;
        while evicted < need_to_evict {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            if self
                .map
                .get(&oldest)
                .map(|d| d.text.is_none())
                .unwrap_or(false)
            {
                self.map.remove(&oldest);
                evicted += 1;
            }
        }
    }

    /// Store new text immediately and return a version token for deferred parsing.
    pub fn set_text(&self, uri: Url, text: String) -> u64 {
        // B4a: mirror into salsa. Done before the DashMap write so downstream
        // salsa queries see the new revision no later than legacy readers.
        self.mirror_text(&uri, &text);

        let mut entry = self.map.entry(uri).or_insert_with(|| Document {
            text: None,
            diagnostics: vec![],
            sem_diagnostics: vec![],
            text_version: 0,
        });
        entry.text_version += 1;
        entry.text = Some(text);
        entry.text_version
    }

    /// Current text revision for `uri`, if the file is known to the store.
    ///
    /// B4d-3c: Backend uses this to gate publication of stale parse
    /// diagnostics. Pattern at the call site:
    /// `if docs.current_version(&uri) == Some(v) { … }`.
    pub fn current_version(&self, uri: &Url) -> Option<u64> {
        self.map.get(uri).map(|d| d.text_version)
    }

    /// Store parse-level diagnostics for `uri`. Always succeeds if the file
    /// is known; the caller is responsible for version gating.
    pub fn set_parse_diagnostics(&self, uri: &Url, diagnostics: Vec<Diagnostic>) {
        if let Some(mut entry) = self.map.get_mut(uri) {
            entry.diagnostics = diagnostics;
        }
    }

    pub fn close(&self, uri: &Url) {
        if let Some(mut entry) = self.map.get_mut(uri) {
            entry.text = None;
            entry.text_version += 1;
            let mut q = self.indexed_order.lock().unwrap();
            if !q.contains(uri) {
                q.push_back(uri.clone());
            }
        }
        self.token_cache.remove(uri);
    }

    pub fn index(&self, uri: Url, text: &str) {
        if self
            .map
            .get(&uri)
            .map(|d| d.text.is_some())
            .unwrap_or(false)
        {
            return;
        }
        // B4a: mirror into salsa before mutating the legacy map.
        self.mirror_text(&uri, text);

        // Phase G1: no eager parse. Salsa's `parsed_doc` query parses lazily
        // on first read. Parse diagnostics are populated by `did_open` when
        // the editor actually opens the file — `get_diagnostics` is only ever
        // read for open files (both call sites in backend.rs gate on
        // `get_doc_salsa`, which returns `None` for background-indexed files).
        self.map.insert(
            uri.clone(),
            Document {
                text: None,
                diagnostics: vec![],
                sem_diagnostics: vec![],
                text_version: 0,
            },
        );

        self.push_to_lru(uri);
    }

    /// Index a file using an already-parsed `ParsedDoc`, avoiding a second parse.
    ///
    /// Prefer this over [`index`] when the caller already has a `ParsedDoc` (e.g.
    /// after running `DefinitionCollector` during workspace scan).
    pub fn index_from_doc(&self, uri: Url, doc: &ParsedDoc, diagnostics: Vec<Diagnostic>) {
        if self
            .map
            .get(&uri)
            .map(|d| d.text.is_some())
            .unwrap_or(false)
        {
            return;
        }
        // B4a: mirror into salsa. `doc.source()` is the text used for this
        // parse, so the salsa revision and the legacy index agree.
        self.mirror_text(&uri, doc.source());

        self.map.insert(
            uri.clone(),
            Document {
                text: None,
                diagnostics,
                sem_diagnostics: vec![],
                text_version: 0,
            },
        );

        self.push_to_lru(uri);
    }

    fn push_to_lru(&self, uri: Url) {
        let mut order = self.indexed_order.lock().unwrap();
        order.push_back(uri);
        // Evict enough indexed-only entries to bring the queue back to DEFAULT_MAX_INDEXED.
        // A file that became open after being indexed must be skipped — it will be
        // re-queued when it is eventually closed.  We must not stop early just
        // because popping an open file decremented order.len() to DEFAULT_MAX_INDEXED;
        // that would leave the map with too many entries.
        let need_to_evict = order
            .len()
            .saturating_sub(self.max_indexed.load(Ordering::Relaxed));
        let mut evicted = 0;
        while evicted < need_to_evict {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            if self
                .map
                .get(&oldest)
                .map(|d| d.text.is_none())
                .unwrap_or(false)
            {
                self.map.remove(&oldest);
                evicted += 1;
            }
            // If the file is open, discard it from the queue and keep looking.
        }
    }

    pub fn remove(&self, uri: &Url) {
        self.map.remove(uri);
        self.token_cache.remove(uri);
        // Also drop the Url→SourceFile mapping so the file stops contributing
        // to the workspace codebase query. Salsa inputs themselves remain
        // alive (salsa doesn't expose input removal in 0.26), but they're
        // orphaned — no query keys them anymore, and re-opening the file
        // allocates a fresh SourceFile with a new FileId. The ~40 bytes per
        // orphan is acceptable; revisit if workspace-churn profiling hurts.
        self.source_files.remove(uri);
        self.text_cache.remove(uri);
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
    /// Matches legacy `get_doc` semantics: returns `Some` only when the file
    /// is currently open in the editor. Background-indexed files get `None`
    /// to preserve feature-handler invariants like "skip if file not open".
    /// Phase B4d will revisit once every caller has been audited.
    #[allow(dead_code)]
    pub fn get_doc_salsa(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        // Gate on legacy open-file state for now.
        let is_open = self.map.get(uri).map(|d| d.text.is_some()).unwrap_or(false);
        if !is_open {
            return None;
        }
        let sf = self.source_file(uri)?;
        Some(self.snapshot_query(move |db| crate::db::parse::parsed_doc(db, sf).0.clone()))
    }

    /// Salsa-backed compact symbol index.
    #[allow(dead_code)]
    pub fn get_index_salsa(&self, uri: &Url) -> Option<Arc<FileIndex>> {
        let sf = self.source_file(uri)?;
        Some(self.snapshot_query(move |db| crate::db::index::file_index(db, sf).0.clone()))
    }

    /// Salsa-backed parsed document, ignoring legacy open-file state.
    ///
    /// Use this for features that legitimately want a `ParsedDoc` for any
    /// mirrored file (open or background-indexed), e.g. call-hierarchy's
    /// index-on-demand path that loads a file from disk, calls `index()`,
    /// then needs to walk the AST. `get_doc_salsa` gates on open-file state
    /// to match legacy semantics; this variant does not.
    #[allow(dead_code)]
    pub fn get_doc_salsa_any(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        let sf = self.source_file(uri)?;
        Some(self.snapshot_query(move |db| crate::db::parse::parsed_doc(db, sf).0.clone()))
    }

    /// Refresh `workspace.files` to mirror the current `source_files` set.
    ///
    /// Called by `get_codebase_salsa`. Skips the setter when the file list
    /// hasn't changed — salsa's `set_field` unconditionally bumps revision,
    /// which would invalidate every downstream query (codebase, file_refs).
    /// Dedup is essential for memoization across LSP requests.
    #[allow(dead_code)] // used by get_codebase_salsa
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

    /// Salsa-backed finalized Codebase. Aggregates every known file's
    /// `StubSlice` via `codebase_from_parts`, memoized by salsa.
    ///
    /// Phase C step 3: this runs in parallel with Backend's imperative
    /// `Arc<Codebase>`. Comparison tests validate parity; readers migrate in
    /// a follow-up.
    #[allow(dead_code)]
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
            crate::db::refs::symbol_refs(db, ws, key.clone())
                .0
                .as_ref()
                .clone()
        })
    }

    /// Salsa-backed per-file method-return-type map.
    #[allow(dead_code)]
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

    /// Returns the live source text (only for open files).
    pub fn get(&self, uri: &Url) -> Option<String> {
        self.map.get(uri).and_then(|d| d.text.clone())
    }

    /// Returns the compact symbol index for a file.
    ///
    /// B4d-3b: the index now comes from salsa's `file_index` query. The
    /// "known to us" gate is expressed by checking membership in `map`,
    /// which still tracks LRU-bounded entries; `remove(uri)` keeps this
    /// consistent by dropping from `map` (while leaving the salsa input
    /// alive for potential re-indexing).
    #[allow(dead_code)]
    pub fn get_index(&self, uri: &Url) -> Option<Arc<FileIndex>> {
        if !self.map.contains_key(uri) {
            return None;
        }
        self.get_index_salsa(uri)
    }

    pub fn get_diagnostics(&self, uri: &Url) -> Option<Vec<Diagnostic>> {
        self.map.get(uri).map(|d| d.diagnostics.clone())
    }

    /// Cache the semantic diagnostics computed by `did_open`/`did_change` so that
    /// `code_action` can read them without holding codebase write locks.
    pub fn set_sem_diagnostics(&self, uri: &Url, diagnostics: Vec<Diagnostic>) {
        if let Some(mut entry) = self.map.get_mut(uri) {
            entry.sem_diagnostics = diagnostics;
        }
    }

    pub fn get_sem_diagnostics(&self, uri: &Url) -> Vec<Diagnostic> {
        self.map
            .get(uri)
            .map(|d| d.sem_diagnostics.clone())
            .unwrap_or_default()
    }

    /// Returns `(uri, doc)` for files currently open in the editor.
    ///
    /// B4d-3a: the ParsedDoc now comes from the salsa `parsed_doc` query
    /// (memoized per input revision); open-state is read from the legacy
    /// map's `text: Option<String>`.
    pub fn all_docs(&self) -> Vec<(Url, Arc<ParsedDoc>)> {
        let open_urls: Vec<Url> = self
            .map
            .iter()
            .filter(|e| e.value().text.is_some())
            .map(|e| e.key().clone())
            .collect();
        open_urls
            .into_iter()
            .filter_map(|u| self.get_doc_salsa_any(&u).map(|d| (u, d)))
            .collect()
    }

    /// Returns `(uri, diagnostics, version)` for every indexed document.
    /// `version` is `None` for non-open files.
    pub fn all_diagnostics(&self) -> Vec<(Url, Vec<Diagnostic>, Option<i64>)> {
        self.map
            .iter()
            .map(|e| {
                let version = if e.value().text.is_some() {
                    Some(e.value().text_version as i64)
                } else {
                    None
                };
                (e.key().clone(), e.value().diagnostics.clone(), version)
            })
            .collect()
    }

    /// Returns `(uri, doc)` first, followed by all other open files.
    /// Use this when the callee requires the primary document to be the first entry.
    pub fn doc_with_others(&self, uri: &Url, doc: Arc<ParsedDoc>) -> Vec<(Url, Arc<ParsedDoc>)> {
        let mut result = vec![(uri.clone(), doc)];
        result.extend(self.other_docs(uri));
        result
    }

    /// Returns `(uri, doc)` for open files excluding `uri`.
    pub fn other_docs(&self, uri: &Url) -> Vec<(Url, Arc<ParsedDoc>)> {
        let open_urls: Vec<Url> = self
            .map
            .iter()
            .filter(|e| e.key() != uri && e.value().text.is_some())
            .map(|e| e.key().clone())
            .collect();
        open_urls
            .into_iter()
            .filter_map(|u| self.get_doc_salsa_any(&u).map(|d| (u, d)))
            .collect()
    }

    /// Batched salsa fetch for open files excluding `uri`: returns each
    /// `(uri, ParsedDoc, MethodReturnsMap)` triple in a single `snapshot_query`
    /// so cancellation retries don't run N times.
    pub fn other_docs_with_returns(
        &self,
        uri: &Url,
    ) -> Vec<(Url, Arc<ParsedDoc>, Arc<crate::ast::MethodReturnsMap>)> {
        let open_urls: Vec<Url> = self
            .map
            .iter()
            .filter(|e| e.key() != uri && e.value().text.is_some())
            .map(|e| e.key().clone())
            .collect();
        let source_files: Vec<(Url, crate::db::input::SourceFile)> = open_urls
            .into_iter()
            .filter_map(|u| self.source_file(&u).map(|sf| (u, sf)))
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

    /// Returns the compact symbol index for every known file.
    ///
    /// B4d-3b: iterates the legacy `map` (still LRU-bounded) and resolves
    /// each index via salsa. Files evicted by LRU are filtered out.
    pub fn all_indexes(&self) -> Vec<(Url, Arc<FileIndex>)> {
        let urls: Vec<Url> = self.map.iter().map(|e| e.key().clone()).collect();
        urls.into_iter()
            .filter_map(|u| self.get_index_salsa(&u).map(|idx| (u, idx)))
            .collect()
    }

    /// Returns indexes for every file except `uri`.
    pub fn other_indexes(&self, uri: &Url) -> Vec<(Url, Arc<FileIndex>)> {
        let urls: Vec<Url> = self
            .map
            .iter()
            .filter(|e| e.key() != uri)
            .map(|e| e.key().clone())
            .collect();
        urls.into_iter()
            .filter_map(|u| self.get_index_salsa(&u).map(|idx| (u, idx)))
            .collect()
    }

    /// Returns parsed documents for every mirrored file, suitable for
    /// full-scan operations (find-references, rename, call_hierarchy,
    /// code_lens).
    ///
    /// B4d-3a: all parsed docs come from salsa's `parsed_doc` query —
    /// memoized per input revision, parsed on demand. Previously this
    /// re-read background files from disk on every call; now salsa caches
    /// the parse across requests.
    pub fn all_docs_for_scan(&self) -> Vec<(Url, Arc<ParsedDoc>)> {
        let urls: Vec<Url> = self.source_files.iter().map(|e| e.key().clone()).collect();
        urls.into_iter()
            .filter_map(|u| self.get_doc_salsa_any(&u).map(|d| (u, d)))
            .collect()
    }

    /// Same as `all_docs_for_scan` but excludes `uri`.
    #[allow(dead_code)]
    pub fn other_docs_for_scan(&self, uri: &Url) -> Vec<(Url, Arc<ParsedDoc>)> {
        let urls: Vec<Url> = self
            .source_files
            .iter()
            .filter(|e| e.key() != uri)
            .map(|e| e.key().clone())
            .collect();
        urls.into_iter()
            .filter_map(|u| self.get_doc_salsa_any(&u).map(|d| (u, d)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    #[test]
    fn get_returns_none_for_unknown_uri() {
        let store = DocumentStore::new();
        assert!(store.get(&uri("/unknown.php")).is_none());
    }

    fn open(store: &DocumentStore, u: Url, text: String) {
        use crate::diagnostics::parse_document;
        let v = store.set_text(u.clone(), text.clone());
        let (_doc, diags) = parse_document(&text);
        assert_eq!(store.current_version(&u), Some(v));
        store.set_parse_diagnostics(&u, diags);
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
    fn open_then_get_returns_text() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php echo 1;".to_string());
        assert_eq!(store.get(&uri("/a.php")).as_deref(), Some("<?php echo 1;"));
    }

    #[test]
    fn update_replaces_text() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php echo 1;".to_string());
        open(&store, uri("/a.php"), "<?php echo 2;".to_string());
        assert_eq!(store.get(&uri("/a.php")).as_deref(), Some("<?php echo 2;"));
    }

    #[test]
    fn close_clears_text_but_keeps_index() {
        let store = DocumentStore::new();
        open(
            &store,
            uri("/a.php"),
            "<?php\nfunction greet() {}".to_string(),
        );
        store.close(&uri("/a.php"));
        assert!(store.get(&uri("/a.php")).is_none());
        // After close, ParsedDoc is gone but FileIndex is retained.
        assert!(store.get_doc_salsa(&uri("/a.php")).is_none());
        assert!(store.get_index(&uri("/a.php")).is_some());
    }

    #[test]
    fn close_nonexistent_uri_is_safe() {
        let store = DocumentStore::new();
        store.close(&uri("/nonexistent.php"));
    }

    #[test]
    fn index_stores_index_without_doc() {
        let store = DocumentStore::new();
        store.index(uri("/lib.php"), "<?php\nfunction lib_fn() {}");
        assert!(store.get(&uri("/lib.php")).is_none());
        // Background-indexed files have no ParsedDoc, only FileIndex.
        assert!(store.get_doc_salsa(&uri("/lib.php")).is_none());
        assert!(store.get_index(&uri("/lib.php")).is_some());
    }

    #[test]
    fn index_does_not_overwrite_open_file() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\n$x = 1;".to_string());
        store.index(uri("/a.php"), "<?php\n$x = 99;");
        assert_eq!(store.get(&uri("/a.php")).as_deref(), Some("<?php\n$x = 1;"));
    }

    #[test]
    fn remove_deletes_entry() {
        let store = DocumentStore::new();
        store.index(uri("/lib.php"), "<?php");
        store.remove(&uri("/lib.php"));
        assert!(store.get_index(&uri("/lib.php")).is_none());
    }

    #[test]
    fn all_docs_only_includes_open_files() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nfunction a() {}".to_string());
        store.index(uri("/b.php"), "<?php\nfunction b() {}");
        // only open files have docs
        assert_eq!(store.all_docs().len(), 1);
    }

    #[test]
    fn all_indexes_includes_all_files() {
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
        open(&store, uri("/a.php"), "<?php\nfunction a() {}".to_string());
        open(&store, uri("/b.php"), "<?php\nfunction b() {}".to_string());
        assert_eq!(store.other_docs(&uri("/a.php")).len(), 1);
    }

    #[test]
    fn open_caches_diagnostics_for_invalid_file() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nclass {".to_string());
        let diags = store.get_diagnostics(&uri("/a.php")).unwrap();
        assert!(!diags.is_empty());
    }

    // ── LRU eviction regression tests ────────────────────────────────────────

    #[test]
    fn eviction_removes_oldest_indexed_file() {
        // Fill the store to exactly DEFAULT_MAX_INDEXED, then add one more.
        // The oldest entry must be evicted so the map stays at DEFAULT_MAX_INDEXED.
        let store = DocumentStore::new();
        for i in 0..DEFAULT_MAX_INDEXED {
            store.index(uri(&format!("/{i}.php")), "<?php");
        }
        store.index(uri("/overflow.php"), "<?php");

        assert_eq!(
            store.all_indexes().len(),
            DEFAULT_MAX_INDEXED,
            "map must not exceed DEFAULT_MAX_INDEXED after overflow"
        );
        assert!(
            store.get_index(&uri("/overflow.php")).is_some(),
            "newly indexed file must be present"
        );
        assert!(
            store.get_index(&uri("/0.php")).is_none(),
            "oldest file must have been evicted"
        );
    }

    #[test]
    fn eviction_skips_open_files_and_evicts_next_indexed() {
        // Regression test for the bug where an open file at the front of the
        // eviction queue caused the loop to exit without evicting anything.
        let store = DocumentStore::new();

        // Index DEFAULT_MAX_INDEXED files; /0.php will be the oldest in the queue.
        for i in 0..DEFAULT_MAX_INDEXED {
            store.index(uri(&format!("/{i}.php")), "<?php");
        }

        // Open /0.php — it now has text and must not be evicted.
        open(&store, uri("/0.php"), "<?php $x = 1;".to_string());

        // Index one more file.  Eviction must skip /0.php (open) and evict
        // /1.php (the next oldest indexed-only file) instead.
        store.index(uri("/overflow.php"), "<?php");

        // The open file must still be present.
        assert!(
            store.get_index(&uri("/0.php")).is_some(),
            "/0.php is open and must not be evicted"
        );
        // The overflow file must have been indexed.
        assert!(
            store.get_index(&uri("/overflow.php")).is_some(),
            "overflow file must be present"
        );
        // The eviction must have brought the map back to DEFAULT_MAX_INDEXED total
        // entries.
        assert_eq!(
            store.all_indexes().len(),
            DEFAULT_MAX_INDEXED,
            "total docs must equal DEFAULT_MAX_INDEXED after eviction"
        );
        // /1.php should have been evicted (oldest indexed-only file after /0.php).
        assert!(
            store.get_index(&uri("/1.php")).is_none(),
            "/1.php must have been evicted as the oldest indexed-only file"
        );
    }

    #[test]
    fn close_evicts_token_cache() {
        let store = DocumentStore::new();
        let u = uri("/a.php");
        open(&store, u.clone(), "<?php".to_string());
        store.store_token_cache(&u, "id1".to_string(), vec![]);
        assert!(store.get_token_cache(&u, "id1").is_some());
        store.close(&u);
        assert!(store.get_token_cache(&u, "id1").is_none());
    }

    #[test]
    fn close_twice_does_not_duplicate_lru_entry() {
        let store = DocumentStore::new();
        let u = uri("/a.php");
        open(&store, u.clone(), "<?php".to_string());
        // First close.
        store.close(&u);
        let len_after_first = store.indexed_order.lock().unwrap().len();
        // Second close — must not push a duplicate.
        store.close(&u);
        let len_after_second = store.indexed_order.lock().unwrap().len();
        assert_eq!(
            len_after_first, len_after_second,
            "second close must not add a duplicate entry to indexed_order"
        );
    }

    #[test]
    fn index_populates_file_index_with_symbols() {
        let store = DocumentStore::new();
        store.index(uri("/a.php"), "<?php\nfunction hello() {}");
        let idx = store.get_index(&uri("/a.php")).unwrap();
        assert_eq!(idx.functions.len(), 1);
        assert_eq!(idx.functions[0].name, "hello");
    }

    #[test]
    fn open_populates_file_index_with_symbols() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nclass Foo {}".to_string());
        let idx = store.get_index(&uri("/a.php")).unwrap();
        assert_eq!(idx.classes.len(), 1);
        assert_eq!(idx.classes[0].name, "Foo");
    }

    // ── B4a mirror invariants ────────────────────────────────────────────
    //
    // Every mutation path that changes file text is supposed to keep the
    // salsa layer in sync with the legacy map. These tests walk an
    // open/edit/close/reopen cycle and assert that the salsa-derived
    // `FileIndex` matches what `get_index` returns at each step.

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
    fn mirror_tracks_set_text_close_reopen() {
        let store = DocumentStore::new();
        let u = uri("/b4a.php");

        // Step 1: set_text -> apply_parse (open flow).
        open(&store, u.clone(), "<?php\nclass A {}".to_string());
        let legacy1 = names_of(&store.get_index(&u).unwrap());
        assert_eq!(legacy1, vec!["A".to_string()]);
        assert_eq!(salsa_index_names(&store, &u), legacy1);

        // Step 2: edit the file.
        open(
            &store,
            u.clone(),
            "<?php\nclass A {}\nclass B {}".to_string(),
        );
        let legacy2 = names_of(&store.get_index(&u).unwrap());
        assert_eq!(legacy2, vec!["A".to_string(), "B".to_string()]);
        assert_eq!(salsa_index_names(&store, &u), legacy2);

        // Step 3: close — legacy drops the AST but keeps FileIndex;
        // salsa keeps the last text and recomputes on demand.
        store.close(&u);
        let legacy3 = names_of(&store.get_index(&u).unwrap());
        assert_eq!(legacy3, legacy2);
        assert_eq!(salsa_index_names(&store, &u), legacy3);

        // Step 4: reopen with different text.
        open(&store, u.clone(), "<?php\nfunction greet() {}".to_string());
        let legacy4 = names_of(&store.get_index(&u).unwrap());
        assert_eq!(legacy4, vec!["greet".to_string()]);
        assert_eq!(salsa_index_names(&store, &u), legacy4);
    }

    #[test]
    fn mirror_tracks_index_and_index_from_doc() {
        let store = DocumentStore::new();

        // Background `index(url, text)` path.
        let u1 = uri("/bg1.php");
        store.index(u1.clone(), "<?php\nclass Bg1 {}");
        assert_eq!(
            salsa_index_names(&store, &u1),
            names_of(&store.get_index(&u1).unwrap())
        );

        // `index_from_doc(url, &doc, diags)` path (workspace-scan Phase 2).
        let u2 = uri("/bg2.php");
        let (doc, diags) =
            crate::diagnostics::parse_document("<?php\nclass Bg2 {}\nfunction f() {}");
        store.index_from_doc(u2.clone(), &doc, diags);
        assert_eq!(
            salsa_index_names(&store, &u2),
            names_of(&store.get_index(&u2).unwrap())
        );
    }

    // ── B4b salsa-backed accessors ─────────────────────────────────────────

    #[test]
    fn get_index_salsa_matches_legacy_get_index() {
        let store = DocumentStore::new();
        let u = uri("/b4b.php");
        open(
            &store,
            u.clone(),
            "<?php\nclass C {}\nfunction h() {}".to_string(),
        );

        let legacy = store.get_index(&u).unwrap();
        let salsa = store.get_index_salsa(&u).unwrap();
        assert_eq!(names_of(&legacy), names_of(&salsa));

        // Edit — both accessors should reflect the new text.
        open(&store, u.clone(), "<?php\nclass Z {}".to_string());
        let legacy2 = store.get_index(&u).unwrap();
        let salsa2 = store.get_index_salsa(&u).unwrap();
        assert_eq!(names_of(&legacy2), names_of(&salsa2));
        assert_eq!(names_of(&salsa2), vec!["Z".to_string()]);
    }

    #[test]
    fn get_doc_salsa_matches_legacy_open_state() {
        let store = DocumentStore::new();
        let u = uri("/b4b_doc.php");

        // Background-index path: legacy returns None; salsa also returns
        // None to preserve "skip if not open" semantics in feature handlers.
        store.index(u.clone(), "<?php\nclass P {}");
        assert!(store.get_doc_salsa(&u).is_none());
        assert!(store.get_doc_salsa(&u).is_none());

        // Now open the same file — both accessors should return Some.
        open(&store, u.clone(), "<?php\nclass P {}".to_string());
        assert!(store.get_doc_salsa(&u).is_some());
        let salsa_doc = store.get_doc_salsa(&u).unwrap();
        assert!(!salsa_doc.program().stmts.is_empty());
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
                        store.set_text(u.clone(), text);
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
}
