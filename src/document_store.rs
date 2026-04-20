use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tower_lsp::lsp_types::{Diagnostic, SemanticToken, Url};

use crate::ast::ParsedDoc;
use crate::diagnostics::parse_document;
use crate::file_index::FileIndex;

/// Default limit used in tests so eviction can be exercised without many files.
#[cfg(test)]
pub(crate) const DEFAULT_MAX_INDEXED: usize = 3;
/// Default maximum number of indexed-only (not open in editor) files kept in memory.
#[cfg(not(test))]
pub(crate) const DEFAULT_MAX_INDEXED: usize = 1_000;

struct Document {
    /// `Some` when the file is open in the editor; `None` for workspace-indexed files.
    text: Option<String>,
    /// `Some` for open files; `None` for background-indexed files (they use `index` instead).
    doc: Option<Arc<ParsedDoc>>,
    /// Always present; compact symbol index extracted after parsing.
    index: Arc<FileIndex>,
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
}

impl Default for DocumentStore {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentStore {
    pub fn new() -> Self {
        DocumentStore {
            map: DashMap::new(),
            indexed_order: Mutex::new(VecDeque::new()),
            token_cache: DashMap::new(),
            max_indexed: AtomicUsize::new(DEFAULT_MAX_INDEXED),
        }
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
        let mut entry = self.map.entry(uri).or_insert_with(|| Document {
            text: None,
            doc: None,
            index: Arc::new(FileIndex::default()),
            diagnostics: vec![],
            sem_diagnostics: vec![],
            text_version: 0,
        });
        entry.text_version += 1;
        entry.text = Some(text);
        entry.text_version
    }

    /// Apply a completed async parse result.
    /// Returns `true` if the update was applied.
    pub fn apply_parse(
        &self,
        uri: &Url,
        doc: ParsedDoc,
        diagnostics: Vec<Diagnostic>,
        version: u64,
    ) -> bool {
        if let Some(mut entry) = self.map.get_mut(uri)
            && entry.text_version == version
        {
            entry.index = Arc::new(FileIndex::extract(uri, &doc));
            entry.doc = Some(Arc::new(doc));
            entry.diagnostics = diagnostics;
            return true;
        }
        false
    }

    pub fn close(&self, uri: &Url) {
        if let Some(mut entry) = self.map.get_mut(uri) {
            entry.text = None;
            // Drop the full ParsedDoc for closed files — the FileIndex is retained.
            entry.doc = None;
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
        // Parse → extract compact index → drop ParsedDoc + text immediately.
        let (index, diagnostics) = {
            let (doc, diagnostics) = parse_document(text);
            let idx = FileIndex::extract(&uri, &doc);
            // `doc` is dropped here — arena freed / returned to pool.
            (Arc::new(idx), diagnostics)
        };

        self.map.insert(
            uri.clone(),
            Document {
                text: None,
                doc: None,
                index,
                diagnostics,
                sem_diagnostics: vec![],
                text_version: 0,
            },
        );

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

    /// Returns the parsed document (cheap Arc clone).
    /// Only `Some` for files currently open in the editor.
    pub fn get_doc(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        self.map.get(uri).and_then(|d| d.doc.clone())
    }

    /// Returns the compact symbol index for a file.
    #[allow(dead_code)]
    pub fn get_index(&self, uri: &Url) -> Option<Arc<FileIndex>> {
        self.map.get(uri).map(|d| d.index.clone())
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
    /// Background-indexed files no longer retain a `ParsedDoc`; use
    /// [`all_docs_for_scan`] when you need to traverse every file's AST.
    pub fn all_docs(&self) -> Vec<(Url, Arc<ParsedDoc>)> {
        self.map
            .iter()
            .filter_map(|e| e.value().doc.as_ref().map(|d| (e.key().clone(), d.clone())))
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
        self.map
            .iter()
            .filter(|e| e.key() != uri)
            .filter_map(|e| e.value().doc.as_ref().map(|d| (e.key().clone(), d.clone())))
            .collect()
    }

    /// Returns the compact symbol index for every indexed file.
    pub fn all_indexes(&self) -> Vec<(Url, Arc<FileIndex>)> {
        self.map
            .iter()
            .map(|e| (e.key().clone(), e.value().index.clone()))
            .collect()
    }

    /// Returns indexes for every file except `uri`.
    pub fn other_indexes(&self, uri: &Url) -> Vec<(Url, Arc<FileIndex>)> {
        self.map
            .iter()
            .filter(|e| e.key() != uri)
            .map(|e| (e.key().clone(), e.value().index.clone()))
            .collect()
    }

    /// Returns parsed documents for every file, suitable for full-scan operations
    /// (find-references, rename, call_hierarchy, code_lens).
    ///
    /// - For open files: returns the in-memory `ParsedDoc` (cheap Arc clone).
    /// - For background-indexed files: re-reads the file from disk and parses it.
    ///   The resulting `ParsedDoc` is **not** stored — callers hold it temporarily.
    pub fn all_docs_for_scan(&self) -> Vec<(Url, Arc<ParsedDoc>)> {
        let mut result = Vec::new();
        for entry in self.map.iter() {
            let uri = entry.key().clone();
            if let Some(doc) = entry.value().doc.as_ref() {
                // Open file: use in-memory doc.
                result.push((uri, doc.clone()));
            } else {
                // Background file: read from disk.
                if let Some(doc) = read_and_parse_from_disk(&uri) {
                    result.push((uri, Arc::new(doc)));
                }
            }
        }
        result
    }

    /// Same as `all_docs_for_scan` but excludes `uri`.
    #[allow(dead_code)]
    pub fn other_docs_for_scan(&self, uri: &Url) -> Vec<(Url, Arc<ParsedDoc>)> {
        let mut result = Vec::new();
        for entry in self.map.iter() {
            let entry_uri = entry.key().clone();
            if &entry_uri == uri {
                continue;
            }
            if let Some(doc) = entry.value().doc.as_ref() {
                result.push((entry_uri, doc.clone()));
            } else {
                if let Some(doc) = read_and_parse_from_disk(&entry_uri) {
                    result.push((entry_uri, Arc::new(doc)));
                }
            }
        }
        result
    }
}

/// Read a file from disk and parse it. Returns `None` if the file cannot be read.
fn read_and_parse_from_disk(uri: &Url) -> Option<ParsedDoc> {
    let _span = tracing::debug_span!("parse_from_disk", file = %uri).entered();
    let path = uri.to_file_path().ok()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let (doc, _) = parse_document(&text);
    Some(doc)
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
        let (doc, diags) = parse_document(&text);
        store.apply_parse(&u, doc, diags, v);
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
        assert!(store.get_doc(&uri("/a.php")).is_none());
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
        assert!(store.get_doc(&uri("/lib.php")).is_none());
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
}
