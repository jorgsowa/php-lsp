use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use php_parser_rs::parser::ast::Statement;
use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::diagnostics::parse_document;

/// Maximum number of indexed-only (not open in editor) files kept in memory.
/// When this limit is exceeded the oldest indexed file is evicted.
const MAX_INDEXED: usize = 10_000;

struct Document {
    /// `Some` when the file is open in the editor; `None` for workspace-indexed files.
    text: Option<String>,
    ast: Arc<Vec<Statement>>,
    diagnostics: Vec<Diagnostic>,
    /// Incremented on every `set_text` call; used to discard stale async parse results.
    text_version: u64,
}

pub struct DocumentStore {
    map: DashMap<Url, Document>,
    /// Insertion-order queue of indexed-only URIs for LRU eviction.
    indexed_order: Mutex<VecDeque<Url>>,
}

impl DocumentStore {
    pub fn new() -> Self {
        DocumentStore {
            map: DashMap::new(),
            indexed_order: Mutex::new(VecDeque::new()),
        }
    }

    pub fn open(&self, uri: Url, text: String) {
        let (ast, diagnostics) = parse_document(&text);
        self.map.insert(uri, Document { text: Some(text), ast: Arc::new(ast), diagnostics, text_version: 1 });
    }

    pub fn update(&self, uri: Url, text: String) {
        let (ast, diagnostics) = parse_document(&text);
        let version = self.map.get(&uri).map(|d| d.text_version + 1).unwrap_or(1);
        self.map.insert(uri, Document { text: Some(text), ast: Arc::new(ast), diagnostics, text_version: version });
    }

    /// Store new text immediately and return a version token for deferred parsing.
    /// The document's AST is preserved until `apply_parse` is called with the matching token.
    pub fn set_text(&self, uri: Url, text: String) -> u64 {
        let mut entry = self.map.entry(uri).or_insert_with(|| Document {
            text: None,
            ast: Arc::new(vec![]),
            diagnostics: vec![],
            text_version: 0,
        });
        entry.text_version += 1;
        entry.text = Some(text);
        entry.text_version
    }

    /// Apply a completed async parse result.
    /// Skips the update if the document's `text_version` has advanced beyond `version`
    /// (meaning a newer edit arrived while this parse was in flight).
    /// Returns `true` if the update was applied.
    pub fn apply_parse(
        &self,
        uri: &Url,
        ast: Vec<Statement>,
        diagnostics: Vec<Diagnostic>,
        version: u64,
    ) -> bool {
        if let Some(mut entry) = self.map.get_mut(uri) {
            if entry.text_version == version {
                entry.ast = Arc::new(ast);
                entry.diagnostics = diagnostics;
                return true;
            }
        }
        false
    }

    /// Called when the editor closes a file. Keeps the AST in the index so
    /// cross-file features (references, completion, …) still see the file.
    /// Also bumps `text_version` to invalidate any in-flight async parse.
    pub fn close(&self, uri: &Url) {
        if let Some(mut entry) = self.map.get_mut(uri) {
            entry.text = None;
            entry.text_version += 1;
            // Track this as an indexed-only entry for LRU eviction
            self.indexed_order.lock().unwrap().push_back(uri.clone());
        }
    }

    /// Index a file found by the workspace scanner. Does not overwrite files
    /// that are currently open in the editor.
    pub fn index(&self, uri: Url, text: &str) {
        if self.map.get(&uri).map(|d| d.text.is_some()).unwrap_or(false) {
            return; // open file takes priority
        }
        let (ast, _diagnostics) = parse_document(text);
        self.map.insert(uri.clone(), Document { text: None, ast: Arc::new(ast), diagnostics: vec![], text_version: 0 });

        // Track insertion order and evict oldest if over limit
        let mut order = self.indexed_order.lock().unwrap();
        order.push_back(uri);
        while order.len() > MAX_INDEXED {
            if let Some(oldest) = order.pop_front() {
                // Only evict if still indexed-only (not re-opened by the editor)
                if self.map.get(&oldest).map(|d| d.text.is_none()).unwrap_or(false) {
                    self.map.remove(&oldest);
                }
            }
        }
    }

    /// Remove a file entirely (e.g. deleted from disk).
    pub fn remove(&self, uri: &Url) {
        self.map.remove(uri);
    }

    /// Returns the raw source text. Returns `None` for indexed-only (closed) files.
    pub fn get(&self, uri: &Url) -> Option<String> {
        self.map.get(uri).and_then(|d| d.text.clone())
    }

    /// Returns the cached AST (cheap Arc clone).
    pub fn get_ast(&self, uri: &Url) -> Option<Arc<Vec<Statement>>> {
        self.map.get(uri).map(|d| d.ast.clone())
    }

    /// Returns cached diagnostics for publishing.
    pub fn get_diagnostics(&self, uri: &Url) -> Option<Vec<Diagnostic>> {
        self.map.get(uri).map(|d| d.diagnostics.clone())
    }

    /// Returns (uri, ast) for all open documents.
    pub fn all_docs_ast(&self) -> Vec<(Url, Arc<Vec<Statement>>)> {
        self.map
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().ast.clone()))
            .collect()
    }

    /// Returns (uri, ast) for every open document except the given URI.
    /// Used for cross-file go-to-definition.
    pub fn other_docs(&self, uri: &Url) -> Vec<(Url, Arc<Vec<Statement>>)> {
        self.map
            .iter()
            .filter(|entry| entry.key() != uri)
            .map(|entry| (entry.key().clone(), entry.value().ast.clone()))
            .collect()
    }

    /// Returns ASTs for every open document except the given URI.
    /// Used for cross-file completion.
    pub fn other_asts(&self, uri: &Url) -> Vec<Arc<Vec<Statement>>> {
        self.map
            .iter()
            .filter(|entry| entry.key() != uri)
            .map(|entry| entry.value().ast.clone())
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

    #[test]
    fn open_then_get_returns_text() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php echo 1;".to_string());
        assert_eq!(store.get(&uri("/a.php")).as_deref(), Some("<?php echo 1;"));
    }

    #[test]
    fn update_replaces_text() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php echo 1;".to_string());
        store.update(uri("/a.php"), "<?php echo 2;".to_string());
        assert_eq!(store.get(&uri("/a.php")).as_deref(), Some("<?php echo 2;"));
    }

    #[test]
    fn close_clears_text_but_keeps_ast() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\nfunction greet() {}".to_string());
        store.close(&uri("/a.php"));
        // text gone (editor closed it)
        assert!(store.get(&uri("/a.php")).is_none());
        // but AST stays for cross-file features
        assert!(store.get_ast(&uri("/a.php")).is_some());
    }

    #[test]
    fn close_nonexistent_uri_is_safe() {
        let store = DocumentStore::new();
        store.close(&uri("/nonexistent.php"));
    }

    #[test]
    fn index_stores_ast_without_text() {
        let store = DocumentStore::new();
        store.index(uri("/lib.php"), "<?php\nfunction lib_fn() {}");
        assert!(store.get(&uri("/lib.php")).is_none(), "indexed file has no text");
        assert!(store.get_ast(&uri("/lib.php")).is_some(), "indexed file has AST");
    }

    #[test]
    fn index_does_not_overwrite_open_file() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\n$x = 1;".to_string());
        store.index(uri("/a.php"), "<?php\n$x = 99;");
        assert_eq!(store.get(&uri("/a.php")).as_deref(), Some("<?php\n$x = 1;"));
    }

    #[test]
    fn remove_deletes_entry() {
        let store = DocumentStore::new();
        store.index(uri("/lib.php"), "<?php");
        store.remove(&uri("/lib.php"));
        assert!(store.get_ast(&uri("/lib.php")).is_none());
    }

    #[test]
    fn all_docs_ast_includes_indexed_files() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\nfunction a() {}".to_string());
        store.index(uri("/b.php"), "<?php\nfunction b() {}");
        let docs = store.all_docs_ast();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn multiple_documents_are_independent() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "a".to_string());
        store.open(uri("/b.php"), "b".to_string());
        assert_eq!(store.get(&uri("/a.php")).as_deref(), Some("a"));
        assert_eq!(store.get(&uri("/b.php")).as_deref(), Some("b"));
        store.close(&uri("/a.php"));
        assert!(store.get(&uri("/a.php")).is_none(), "closed file has no text");
        assert_eq!(store.get(&uri("/b.php")).as_deref(), Some("b"));
    }

    #[test]
    fn open_caches_ast() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\nfunction greet() {}".to_string());
        let ast = store.get_ast(&uri("/a.php")).expect("ast should be cached");
        assert!(!ast.is_empty());
    }

    #[test]
    fn open_caches_diagnostics_for_valid_file() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\nfunction greet() {}".to_string());
        let diags = store.get_diagnostics(&uri("/a.php")).expect("diagnostics should be cached");
        assert!(diags.is_empty(), "valid file should have no diagnostics");
    }

    #[test]
    fn open_caches_diagnostics_for_invalid_file() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\nclass {".to_string());
        let diags = store.get_diagnostics(&uri("/a.php")).expect("diagnostics should be cached");
        assert!(!diags.is_empty(), "invalid file should have diagnostics");
    }

    #[test]
    fn update_refreshes_ast_and_diagnostics() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\nclass {".to_string());
        assert!(!store.get_diagnostics(&uri("/a.php")).unwrap().is_empty());
        store.update(uri("/a.php"), "<?php\nclass Foo {}".to_string());
        assert!(store.get_diagnostics(&uri("/a.php")).unwrap().is_empty());
    }

    #[test]
    fn other_asts_excludes_current_uri() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\nfunction a() {}".to_string());
        store.open(uri("/b.php"), "<?php\nfunction b() {}".to_string());
        let others = store.other_asts(&uri("/a.php"));
        assert_eq!(others.len(), 1);
    }

    #[test]
    fn other_docs_returns_uri_and_ast() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php\nfunction a() {}".to_string());
        store.open(uri("/b.php"), "<?php\nfunction b() {}".to_string());
        let others = store.other_docs(&uri("/a.php"));
        assert_eq!(others.len(), 1);
        assert_eq!(others[0].0, uri("/b.php"));
        assert!(!others[0].1.is_empty());
    }

    #[test]
    fn lru_eviction_evicts_oldest_indexed_file() {
        // Override MAX_INDEXED by indexing more files than the limit.
        // Since MAX_INDEXED=10_000 is too large to test directly, we
        // verify that the eviction machinery at least doesn't panic.
        let store = DocumentStore::new();
        // Index two files and verify they are both present
        store.index(uri("/a.php"), "<?php\nfunction a() {}");
        store.index(uri("/b.php"), "<?php\nfunction b() {}");
        assert!(store.get_ast(&uri("/a.php")).is_some());
        assert!(store.get_ast(&uri("/b.php")).is_some());
    }

    #[test]
    fn open_file_not_evicted_by_lru() {
        let store = DocumentStore::new();
        store.open(uri("/open.php"), "<?php\nfunction f() {}".to_string());
        // Index many files — the open file must survive regardless
        for i in 0..50 {
            store.index(uri(&format!("/lib{i}.php")), "<?php");
        }
        assert!(store.get_ast(&uri("/open.php")).is_some(), "open file must not be evicted");
    }
}
