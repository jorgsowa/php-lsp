// Reached only through the server, which is not built for wasm.
#![cfg_attr(target_family = "wasm", allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use tower_lsp_server::ls_types::{Diagnostic, Uri};

use crate::analysis::semantic_diagnostics::issues_to_diagnostics_gated;
use crate::document::ast::ParsedDoc;
use crate::document::document_store::DocumentStore;
use crate::lang::config::DiagnosticsConfig;

/// Per-open-file state owned by `Backend` (Phase E4).
///
/// Previously this lived inside `DocumentStore`'s `map: DashMap<Uri, Document>`,
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
    /// Most recent external-tool (PHPStan/PHPCS) findings, cached so every
    /// publish path can append them without re-running the tools.
    pub(crate) external_diagnostics: Vec<Diagnostic>,
    /// `version` this file was at when `external_diagnostics` was computed.
    /// An edit bumps `version` without touching this field, so a mismatch
    /// means the cached findings were computed against text the buffer no
    /// longer holds — [`OpenFiles::external_diagnostics`] treats that as
    /// stale and returns nothing rather than showing diagnostics against the
    /// wrong line numbers.
    pub(crate) external_diagnostics_version: Option<u64>,
}

/// Shared handle to open-file state. Cheaply cloneable — wraps an `Arc<DashMap>`
/// so it can be captured by async closures alongside `Arc<DocumentStore>`.
#[derive(Clone, Default)]
pub struct OpenFiles {
    files: Arc<DashMap<Uri, OpenFile>>,
    /// The most recent open/close intent accepted from the transport for a
    /// URI. Tower processes requests concurrently, so a `didClose` can begin
    /// after the following `didOpen`; retaining the ingress intent prevents
    /// that stale close from erasing the reopened buffer.
    lifecycle_intents: Arc<DashMap<Uri, LifecycleIntent>>,
    next_lifecycle_generation: Arc<AtomicU64>,
}

#[derive(Clone, Copy)]
struct LifecycleIntent {
    generation: u64,
    is_open: bool,
    in_flight: usize,
}

impl OpenFiles {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set_open_text(&self, docs: &DocumentStore, uri: Uri, text: String) -> u64 {
        // Build the Arc once and hand the same allocation to both the salsa
        // mirror and the open-file entry — `mirror_text_arc` skips the extra
        // `Arc::from` copy `mirror_text` would otherwise make from `&text`.
        let text: Arc<str> = Arc::from(text);
        docs.mirror_text_arc(&uri, Arc::clone(&text));
        let mut entry = self.files.entry(uri).or_default();
        entry.version += 1;
        entry.text = text;
        entry.version
    }

    pub(crate) fn close(&self, docs: &DocumentStore, uri: &Uri) {
        self.files.remove(uri);
        docs.evict_token_cache(uri);
    }

    pub(crate) fn current_version(&self, uri: &Uri) -> Option<u64> {
        self.files.get(uri).map(|e| e.version)
    }

    pub(crate) fn text(&self, uri: &Uri) -> Option<Arc<str>> {
        self.files.get(uri).map(|e| Arc::clone(&e.text))
    }

    pub(crate) fn set_parse_diagnostics(&self, uri: &Uri, diagnostics: Vec<Diagnostic>) {
        if let Some(mut entry) = self.files.get_mut(uri) {
            entry.parse_diagnostics = diagnostics;
        }
    }

    pub(crate) fn parse_diagnostics(&self, uri: &Uri) -> Option<Vec<Diagnostic>> {
        self.files.get(uri).map(|e| e.parse_diagnostics.clone())
    }

    /// Record the content hash of a `publishDiagnostics` just sent for `uri`.
    /// No-op when the file closed mid-flight (entry gone).
    pub(crate) fn note_published(&self, uri: &Uri, hash: u64) {
        if let Some(mut entry) = self.files.get_mut(uri) {
            entry.published_hash = Some(hash);
        }
    }

    /// Hash of the last publish sent for `uri`, if it is still open.
    pub(crate) fn published_hash(&self, uri: &Uri) -> Option<u64> {
        self.files.get(uri).and_then(|e| e.published_hash)
    }

    /// Cache `diagnostics` from an external tool run against `uri` at
    /// `version`. No-op when the file closed mid-run (entry gone).
    pub(crate) fn set_external_diagnostics(
        &self,
        uri: &Uri,
        version: u64,
        diagnostics: Vec<Diagnostic>,
    ) {
        if let Some(mut entry) = self.files.get_mut(uri) {
            entry.external_diagnostics = diagnostics;
            entry.external_diagnostics_version = Some(version);
        }
    }

    /// Cached external-tool diagnostics for `uri`, or empty if none have
    /// been computed yet or the buffer has changed since they were.
    pub(crate) fn external_diagnostics(&self, uri: &Uri) -> Vec<Diagnostic> {
        self.files
            .get(uri)
            .filter(|e| e.external_diagnostics_version == Some(e.version))
            .map(|e| e.external_diagnostics.clone())
            .unwrap_or_default()
    }

    pub(crate) fn all_with_versions(&self) -> Vec<(Uri, Option<i64>)> {
        self.files
            .iter()
            .map(|e| (e.key().clone(), Some(e.value().version as i64)))
            .collect()
    }

    pub(crate) fn urls(&self) -> Vec<Uri> {
        self.files.iter().map(|e| e.key().clone()).collect()
    }

    pub(crate) fn contains(&self, uri: &Uri) -> bool {
        self.files.contains_key(uri)
    }

    /// Record lifecycle intent at transport ingress, before tower schedules
    /// the async notification handler.
    pub(crate) fn note_lifecycle_intent(&self, uri: Uri, is_open: bool) -> u64 {
        let generation = self
            .next_lifecycle_generation
            .fetch_add(1, Ordering::Relaxed);
        self.lifecycle_intents
            .entry(uri)
            .and_modify(|intent| {
                intent.generation = generation;
                intent.is_open = is_open;
                intent.in_flight += 1;
            })
            .or_insert(LifecycleIntent {
                generation,
                is_open,
                in_flight: 1,
            });
        generation
    }

    pub(crate) fn finish_lifecycle_intent(&self, uri: &Uri) {
        let should_remove = self
            .lifecycle_intents
            .get_mut(uri)
            .is_some_and(|mut intent| {
                intent.in_flight -= 1;
                intent.in_flight == 0
            });
        if should_remove {
            self.lifecycle_intents
                .remove_if(uri, |_, intent| intent.in_flight == 0);
        }
    }

    /// Whether this exact lifecycle notification is still the newest intent
    /// accepted from the transport for `uri`.
    pub(crate) fn should_apply_lifecycle(&self, uri: &Uri, generation: u64, is_open: bool) -> bool {
        self.lifecycle_intents
            .get(uri)
            .is_some_and(|intent| intent.generation == generation && intent.is_open == is_open)
    }

    pub(crate) fn lifecycle_intent_count(&self) -> usize {
        self.lifecycle_intents.len()
    }

    /// Open-gated parsed doc: returns `Some` only when `uri` is currently open.
    pub(crate) fn get_doc(&self, docs: &DocumentStore, uri: &Uri) -> Option<Arc<ParsedDoc>> {
        if !self.contains(uri) {
            return None;
        }
        docs.get_doc_salsa(uri)
    }

    /// Open-gated, stale-tolerant parsed doc for cursor-triggered cosmetic reads
    /// (`documentHighlight`). Never spins under write pressure — see
    /// [`DocumentStore::get_doc_snapshot_or_stale`].
    pub(crate) fn get_doc_stale(&self, docs: &DocumentStore, uri: &Uri) -> Option<Arc<ParsedDoc>> {
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
/// Before the initial workspace scan finishes (`!docs.is_index_ready()`),
/// symbol lookups against the workspace-wide table only see the subset of
/// files indexed so far, so issues that depend on such a lookup (e.g.
/// `UndefinedClass` for a global class declared in a file not yet reached
/// by the scan) can misreport a real declaration as missing — see issue
/// #242. Those specific kinds are held back until the index is ready (the
/// caller republishes once it is, see `republish_after_index_ready`); issues
/// checked purely against the file's own contents are unaffected and always
/// reported immediately.
pub(crate) fn compute_open_file_diagnostics(
    docs: &DocumentStore,
    open_files: &OpenFiles,
    uri: &Uri,
    diag_cfg: &DiagnosticsConfig,
    is_laravel: bool,
) -> Vec<Diagnostic> {
    if crate::index::matches_any(uri.path().as_str(), &diag_cfg.exclude_paths) {
        return Vec::new();
    }
    let mut out = open_files.parse_diagnostics(uri).unwrap_or_default();
    if let Some(issues) = docs.get_semantic_issues_salsa(uri) {
        out.extend(issues_to_diagnostics_gated(
            &issues,
            uri,
            diag_cfg,
            docs.is_index_ready(),
        ));
    }
    out.extend(crate::laravel::unguarded_model_diagnostics(
        docs, uri, is_laravel,
    ));
    out.extend(open_files.external_diagnostics(uri));
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
        let uri = ("file:///test.php").parse::<Uri>().unwrap();
        open_files.set_open_text(&docs, uri.clone(), "<?php".to_owned());

        let first = open_files.text(&uri).unwrap();
        let second = open_files.text(&uri).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "repeated reads must share one allocation, not clone the buffer per call"
        );
    }

    #[test]
    fn reopen_supersedes_an_earlier_close_at_transport_ingress() {
        let open_files = OpenFiles::new();
        let uri = "file:///reopen.php".parse::<Uri>().unwrap();

        let close_generation = open_files.note_lifecycle_intent(uri.clone(), false);
        let reopen_generation = open_files.note_lifecycle_intent(uri.clone(), true);

        assert!(open_files.should_apply_lifecycle(&uri, reopen_generation, true));
        assert!(
            !open_files.should_apply_lifecycle(&uri, close_generation, false),
            "a close received before a reopen must not apply after it"
        );
        open_files.finish_lifecycle_intent(&uri);
        open_files.finish_lifecycle_intent(&uri);
        assert_eq!(open_files.lifecycle_intent_count(), 0);
    }

    /// Issue #242: before the workspace index is ready, a reference to a
    /// class the store hasn't seen yet is indistinguishable from a genuine
    /// typo — `UndefinedClass` must be held back either way. Once
    /// `mark_index_ready` flips, the same (still-unresolvable) reference must
    /// be reported, so this isn't just permanently swallowed.
    #[test]
    fn undefined_class_suppressed_until_index_ready() {
        let docs = DocumentStore::new();
        let open_files = OpenFiles::new();
        let uri = ("file:///app.php").parse::<Uri>().unwrap();
        open_files.set_open_text(
            &docs,
            uri.clone(),
            "<?php\n\nnew TrulyMissingClass();\n".to_owned(),
        );
        let diag_cfg = DiagnosticsConfig::default();

        let before = compute_open_file_diagnostics(&docs, &open_files, &uri, &diag_cfg, false);
        assert!(
            before.is_empty(),
            "UndefinedClass must be suppressed before the index is ready, got: {before:?}"
        );

        docs.mark_index_ready();
        let after = compute_open_file_diagnostics(&docs, &open_files, &uri, &diag_cfg, false);
        assert!(
            after
                .iter()
                .any(|d| d.message.contains("TrulyMissingClass")),
            "UndefinedClass must be reported once the index is ready, got: {after:?}"
        );
    }

    /// A diagnostic checked purely against the file's own contents (a
    /// same-function return-type mismatch) must never wait on the workspace
    /// index — only symbol lookups against the workspace-wide table are
    /// index-ready-gated (see `issue_needs_workspace_index`).
    #[test]
    fn local_only_issue_not_suppressed_before_index_ready() {
        let docs = DocumentStore::new();
        let open_files = OpenFiles::new();
        let uri = ("file:///app.php").parse::<Uri>().unwrap();
        open_files.set_open_text(
            &docs,
            uri.clone(),
            "<?php\nfunction f(): string { return 42; }\n".to_owned(),
        );
        let diag_cfg = DiagnosticsConfig::default();

        let before = compute_open_file_diagnostics(&docs, &open_files, &uri, &diag_cfg, false);
        assert!(
            !before.is_empty(),
            "a same-file return-type mismatch must not wait on the workspace index"
        );
    }

    #[test]
    fn exclude_paths_suppresses_all_diagnostics_for_matching_files() {
        let docs = DocumentStore::new();
        let open_files = OpenFiles::new();
        let src = "<?php\nfunction f(): string { return 42; }\n";
        let vendor = ("file:///proj/vendor/acme/Lib.php").parse::<Uri>().unwrap();
        let app = ("file:///proj/src/App.php").parse::<Uri>().unwrap();
        open_files.set_open_text(&docs, vendor.clone(), src.to_owned());
        open_files.set_open_text(&docs, app.clone(), src.to_owned());
        open_files.set_parse_diagnostics(&vendor, vec![Diagnostic::default()]);
        let diag_cfg = DiagnosticsConfig {
            exclude_paths: vec!["vendor/*".to_string()],
            ..DiagnosticsConfig::default()
        };

        assert!(
            compute_open_file_diagnostics(&docs, &open_files, &vendor, &diag_cfg, false).is_empty()
        );
        assert!(
            !compute_open_file_diagnostics(&docs, &open_files, &app, &diag_cfg, false).is_empty()
        );
    }
}
