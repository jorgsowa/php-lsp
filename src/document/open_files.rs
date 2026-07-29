use std::sync::Arc;

use dashmap::DashMap;
use tower_lsp::lsp_types::{Diagnostic, Url};

use crate::analysis::semantic_diagnostics::issues_to_diagnostics;
use crate::document::ast::ParsedDoc;
use crate::document::document_store::DocumentStore;
use crate::lang::config::DiagnosticsConfig;

/// Per-open-file state owned by `Backend` (Phase E4).
///
/// Previously this lived inside `DocumentStore`'s `map: DashMap<Url, Document>`,
/// but none of these fields are salsa-shaped: `text` is the live editor buffer,
/// `version` is an async-parse gate, and `parse_diagnostics` is a publish cache.
/// Keeping them on `Backend` leaves `DocumentStore` as a pure salsa-input wrapper.
#[derive(Default, Clone)]
pub(crate) struct OpenFile {
    /// Live editor text. `Arc`-backed so readers (`OpenFiles::text`, hit on
    /// every request that needs the current buffer) clone a refcount instead
    /// of the whole buffer.
    pub(crate) text: Arc<str>,
    /// Monotonic counter bumped on every `set_open_text` / `close_open_file`;
    /// used to discard stale async parse results.
    pub(crate) version: u64,
    /// Parse-level diagnostics most recently cached for publication.
    pub(crate) parse_diagnostics: Vec<Diagnostic>,
    /// Content hash of the last `publishDiagnostics` sent for this file.
    /// Lets the dependent-republish sweep skip publishes whose content the
    /// client already displays. `None` until the first publish; the entry
    /// (and hash) drops on close, so a reopen always publishes.
    pub(crate) published_hash: Option<u64>,
}

/// Shared handle to open-file state. Cheaply cloneable — wraps an `Arc<DashMap>`
/// so it can be captured by async closures alongside `Arc<DocumentStore>`.
#[derive(Clone, Default)]
pub struct OpenFiles(Arc<DashMap<Url, OpenFile>>);

impl OpenFiles {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_open_text(&self, docs: &DocumentStore, uri: Url, text: String) -> u64 {
        // Build the Arc once and hand the same allocation to both the salsa
        // mirror and the open-file entry — `mirror_text_arc` skips the extra
        // `Arc::from` copy `mirror_text` would otherwise make from `&text`.
        let text: Arc<str> = Arc::from(text);
        docs.mirror_text_arc(&uri, Arc::clone(&text));
        let mut entry = self.0.entry(uri).or_default();
        entry.version += 1;
        entry.text = text;
        entry.version
    }

    pub(crate) fn close(&self, docs: &DocumentStore, uri: &Url) {
        self.0.remove(uri);
        docs.evict_token_cache(uri);
    }

    pub(crate) fn current_version(&self, uri: &Url) -> Option<u64> {
        self.0.get(uri).map(|e| e.version)
    }

    pub(crate) fn text(&self, uri: &Url) -> Option<Arc<str>> {
        self.0.get(uri).map(|e| Arc::clone(&e.text))
    }

    pub(crate) fn set_parse_diagnostics(&self, uri: &Url, diagnostics: Vec<Diagnostic>) {
        if let Some(mut entry) = self.0.get_mut(uri) {
            entry.parse_diagnostics = diagnostics;
        }
    }

    pub(crate) fn parse_diagnostics(&self, uri: &Url) -> Option<Vec<Diagnostic>> {
        self.0.get(uri).map(|e| e.parse_diagnostics.clone())
    }

    /// Record the content hash of a `publishDiagnostics` just sent for `uri`.
    /// No-op when the file closed mid-flight (entry gone).
    pub(crate) fn note_published(&self, uri: &Url, hash: u64) {
        if let Some(mut entry) = self.0.get_mut(uri) {
            entry.published_hash = Some(hash);
        }
    }

    /// Hash of the last publish sent for `uri`, if it is still open.
    pub(crate) fn published_hash(&self, uri: &Url) -> Option<u64> {
        self.0.get(uri).and_then(|e| e.published_hash)
    }

    pub(crate) fn all_with_diagnostics(&self) -> Vec<(Url, Vec<Diagnostic>, Option<i64>)> {
        self.0
            .iter()
            .map(|e| {
                (
                    e.key().clone(),
                    e.value().parse_diagnostics.clone(),
                    Some(e.value().version as i64),
                )
            })
            .collect()
    }

    pub(crate) fn urls(&self) -> Vec<Url> {
        self.0.iter().map(|e| e.key().clone()).collect()
    }

    pub(crate) fn contains(&self, uri: &Url) -> bool {
        self.0.contains_key(uri)
    }

    /// Open-gated parsed doc: returns `Some` only when `uri` is currently open.
    pub(crate) fn get_doc(&self, docs: &DocumentStore, uri: &Url) -> Option<Arc<ParsedDoc>> {
        if !self.contains(uri) {
            return None;
        }
        docs.get_doc_salsa(uri)
    }

    /// Open-gated, stale-tolerant parsed doc for cursor-triggered cosmetic reads
    /// (`documentHighlight`). Never spins under write pressure — see
    /// [`DocumentStore::get_doc_snapshot_or_stale`].
    pub(crate) fn get_doc_stale(&self, docs: &DocumentStore, uri: &Url) -> Option<Arc<ParsedDoc>> {
        if !self.contains(uri) {
            return None;
        }
        docs.get_doc_snapshot_or_stale(uri)
    }
}

/// Build the full diagnostic bundle for an already-open file.
///
/// Reuses cached parse diagnostics from `OpenFiles` (set by the file's own
/// debounced parse) and recomputes the rest. `semantic_issues` is
/// salsa-cached; for files unaffected by the triggering change it's a cache
/// hit.
///
/// Used both for the originating file (during `did_open`/`did_change`) and
/// when proactively republishing diagnostics to other open files after a
/// dependency edit. Salsa-blocking — call from a `spawn_blocking` if invoked
/// off the originating file's debounce path.
pub(crate) fn compute_open_file_diagnostics(
    docs: &DocumentStore,
    open_files: &OpenFiles,
    uri: &Url,
    diag_cfg: &DiagnosticsConfig,
) -> Vec<Diagnostic> {
    let mut out = open_files.parse_diagnostics(uri).unwrap_or_default();
    if let Some(issues) = docs.get_semantic_issues_salsa(uri) {
        out.extend(issues_to_diagnostics(&issues, uri, diag_cfg));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `OpenFiles::text` must hand out the same `Arc` allocation on repeat
    /// reads (a refcount bump), not a fresh clone of the buffer — the whole
    /// point of storing `Arc<str>` instead of `String` in `OpenFile`.
    #[test]
    fn text_reuses_the_same_arc_across_reads() {
        let docs = DocumentStore::new();
        let open_files = OpenFiles::new();
        let uri = Url::parse("file:///test.php").unwrap();
        open_files.set_open_text(&docs, uri.clone(), "<?php".to_owned());

        let first = open_files.text(&uri).unwrap();
        let second = open_files.text(&uri).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "repeated reads must share one allocation, not clone the buffer per call"
        );
    }
}
