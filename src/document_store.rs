use dashmap::DashMap;
use tower_lsp::lsp_types::Url;

pub struct DocumentStore(DashMap<Url, String>);

impl DocumentStore {
    pub fn new() -> Self {
        DocumentStore(DashMap::new())
    }

    pub fn open(&self, uri: Url, text: String) {
        self.0.insert(uri, text);
    }

    pub fn update(&self, uri: Url, text: String) {
        self.0.insert(uri, text);
    }

    pub fn close(&self, uri: &Url) {
        self.0.remove(uri);
    }

    pub fn get(&self, uri: &Url) -> Option<String> {
        self.0.get(uri).map(|v| v.clone())
    }
}
