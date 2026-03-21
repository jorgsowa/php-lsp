use std::sync::Arc;

use dashmap::DashMap;
use php_parser_rs::parser::ast::Statement;
use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::diagnostics::parse_document;

struct Document {
    text: String,
    ast: Arc<Vec<Statement>>,
    diagnostics: Vec<Diagnostic>,
}

pub struct DocumentStore(DashMap<Url, Document>);

impl DocumentStore {
    pub fn new() -> Self {
        DocumentStore(DashMap::new())
    }

    pub fn open(&self, uri: Url, text: String) {
        let (ast, diagnostics) = parse_document(&text);
        self.0.insert(uri, Document { text, ast: Arc::new(ast), diagnostics });
    }

    pub fn update(&self, uri: Url, text: String) {
        let (ast, diagnostics) = parse_document(&text);
        self.0.insert(uri, Document { text, ast: Arc::new(ast), diagnostics });
    }

    pub fn close(&self, uri: &Url) {
        self.0.remove(uri);
    }

    /// Returns the raw source text.
    pub fn get(&self, uri: &Url) -> Option<String> {
        self.0.get(uri).map(|d| d.text.clone())
    }

    /// Returns the cached AST (cheap Arc clone).
    pub fn get_ast(&self, uri: &Url) -> Option<Arc<Vec<Statement>>> {
        self.0.get(uri).map(|d| d.ast.clone())
    }

    /// Returns cached diagnostics for publishing.
    pub fn get_diagnostics(&self, uri: &Url) -> Option<Vec<Diagnostic>> {
        self.0.get(uri).map(|d| d.diagnostics.clone())
    }

    /// Returns (uri, ast) for every open document except the given URI.
    /// Used for cross-file go-to-definition.
    pub fn other_docs(&self, uri: &Url) -> Vec<(Url, Arc<Vec<Statement>>)> {
        self.0
            .iter()
            .filter(|entry| entry.key() != uri)
            .map(|entry| (entry.key().clone(), entry.value().ast.clone()))
            .collect()
    }

    /// Returns ASTs for every open document except the given URI.
    /// Used for cross-file completion.
    pub fn other_asts(&self, uri: &Url) -> Vec<Arc<Vec<Statement>>> {
        self.0
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
    fn close_removes_document() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "<?php".to_string());
        store.close(&uri("/a.php"));
        assert!(store.get(&uri("/a.php")).is_none());
    }

    #[test]
    fn close_nonexistent_uri_is_safe() {
        let store = DocumentStore::new();
        store.close(&uri("/nonexistent.php"));
    }

    #[test]
    fn multiple_documents_are_independent() {
        let store = DocumentStore::new();
        store.open(uri("/a.php"), "a".to_string());
        store.open(uri("/b.php"), "b".to_string());
        assert_eq!(store.get(&uri("/a.php")).as_deref(), Some("a"));
        assert_eq!(store.get(&uri("/b.php")).as_deref(), Some("b"));
        store.close(&uri("/a.php"));
        assert!(store.get(&uri("/a.php")).is_none());
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
}
