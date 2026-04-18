use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tower_lsp::lsp_types::{Diagnostic, SemanticToken, Url};

use crate::ast::ParsedDoc;
use crate::diagnostics::parse_document;

/// Maximum number of indexed-only (not open in editor) files kept in memory.
#[cfg(not(test))]
const MAX_INDEXED: usize = 10_000;
/// Reduced limit used in tests so eviction can be exercised without 10 k files.
#[cfg(test)]
const MAX_INDEXED: usize = 3;

struct Document {
    /// `Some` when the file is open in the editor; `None` for workspace-indexed files.
    text: Option<String>,
    doc: Arc<ParsedDoc>,
    diagnostics: Vec<Diagnostic>,
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
        }
    }

    /// Store new text immediately and return a version token for deferred parsing.
    pub fn set_text(&self, uri: Url, text: String) -> u64 {
        let mut entry = self.map.entry(uri).or_insert_with(|| Document {
            text: None,
            doc: Arc::new(ParsedDoc::default()),
            diagnostics: vec![],
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
            entry.doc = Arc::new(doc);
            entry.diagnostics = diagnostics;
            return true;
        }
        false
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
        let (doc, diagnostics) = parse_document(text);
        self.map.insert(
            uri.clone(),
            Document {
                text: None,
                doc: Arc::new(doc),
                diagnostics,
                text_version: 0,
            },
        );

        let mut order = self.indexed_order.lock().unwrap();
        order.push_back(uri);
        // Evict enough indexed-only entries to bring the queue back to MAX_INDEXED.
        // A file that became open after being indexed must be skipped — it will be
        // re-queued when it is eventually closed.  We must not stop early just
        // because popping an open file decremented order.len() to MAX_INDEXED;
        // that would leave the map with too many entries.
        let need_to_evict = order.len().saturating_sub(MAX_INDEXED);
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

    /// Returns the parsed document (cheap Arc clone). Always present once indexed.
    pub fn get_doc(&self, uri: &Url) -> Option<Arc<ParsedDoc>> {
        self.map.get(uri).map(|d| d.doc.clone())
    }

    pub fn get_diagnostics(&self, uri: &Url) -> Option<Vec<Diagnostic>> {
        self.map.get(uri).map(|d| d.diagnostics.clone())
    }

    pub fn all_docs(&self) -> Vec<(Url, Arc<ParsedDoc>)> {
        self.map
            .iter()
            .map(|e| (e.key().clone(), e.value().doc.clone()))
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

    pub fn other_docs(&self, uri: &Url) -> Vec<(Url, Arc<ParsedDoc>)> {
        self.map
            .iter()
            .filter(|e| e.key() != uri)
            .map(|e| (e.key().clone(), e.value().doc.clone()))
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
    fn close_clears_text_but_keeps_doc() {
        let store = DocumentStore::new();
        open(
            &store,
            uri("/a.php"),
            "<?php\nfunction greet() {}".to_string(),
        );
        store.close(&uri("/a.php"));
        assert!(store.get(&uri("/a.php")).is_none());
        assert!(store.get_doc(&uri("/a.php")).is_some());
    }

    #[test]
    fn close_nonexistent_uri_is_safe() {
        let store = DocumentStore::new();
        store.close(&uri("/nonexistent.php"));
    }

    #[test]
    fn index_stores_doc_without_text() {
        let store = DocumentStore::new();
        store.index(uri("/lib.php"), "<?php\nfunction lib_fn() {}");
        assert!(store.get(&uri("/lib.php")).is_none());
        assert!(store.get_doc(&uri("/lib.php")).is_some());
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
        assert!(store.get_doc(&uri("/lib.php")).is_none());
    }

    #[test]
    fn all_docs_includes_indexed_files() {
        let store = DocumentStore::new();
        open(&store, uri("/a.php"), "<?php\nfunction a() {}".to_string());
        store.index(uri("/b.php"), "<?php\nfunction b() {}");
        assert_eq!(store.all_docs().len(), 2);
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
        // Fill the store to exactly MAX_INDEXED, then add one more.
        // The oldest entry must be evicted so the map stays at MAX_INDEXED.
        let store = DocumentStore::new();
        for i in 0..MAX_INDEXED {
            store.index(uri(&format!("/{i}.php")), "<?php");
        }
        store.index(uri("/overflow.php"), "<?php");

        assert_eq!(
            store.all_docs().len(),
            MAX_INDEXED,
            "map must not exceed MAX_INDEXED after overflow"
        );
        assert!(
            store.get_doc(&uri("/overflow.php")).is_some(),
            "newly indexed file must be present"
        );
        assert!(
            store.get_doc(&uri("/0.php")).is_none(),
            "oldest file must have been evicted"
        );
    }

    #[test]
    fn eviction_skips_open_files_and_evicts_next_indexed() {
        // Regression test for the bug where an open file at the front of the
        // eviction queue caused the loop to exit without evicting anything:
        //
        //   order.len() was MAX_INDEXED+1 → pop open file → order.len() drops
        //   to MAX_INDEXED → while condition false → loop exits → no eviction.
        //
        // After the fix the loop tracks `need_to_evict` independently of
        // order.len(), so it keeps looking until it finds an indexed file.
        let store = DocumentStore::new();

        // Index MAX_INDEXED files; /0.php will be the oldest in the queue.
        for i in 0..MAX_INDEXED {
            store.index(uri(&format!("/{i}.php")), "<?php");
        }

        // Open /0.php — it now has text and must not be evicted.
        open(&store, uri("/0.php"), "<?php $x = 1;".to_string());

        // Index one more file.  Eviction must skip /0.php (open) and evict
        // /1.php (the next oldest indexed-only file) instead.
        store.index(uri("/overflow.php"), "<?php");

        // The open file must still be present.
        assert!(
            store.get_doc(&uri("/0.php")).is_some(),
            "/0.php is open and must not be evicted"
        );
        // The overflow file must have been indexed.
        assert!(
            store.get_doc(&uri("/overflow.php")).is_some(),
            "overflow file must be present"
        );
        // The eviction must have brought the map back to MAX_INDEXED total
        // entries: /0.php (open) + the remaining indexed files + /overflow.php.
        assert_eq!(
            store.all_docs().len(),
            MAX_INDEXED,
            "total docs must equal MAX_INDEXED after eviction"
        );
        // /1.php should have been evicted (oldest indexed-only file after /0.php).
        assert!(
            store.get_doc(&uri("/1.php")).is_none(),
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
}
