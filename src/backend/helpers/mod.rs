//! Backend support helpers, grouped by concern:
//! - [`position`] — character/offset math and the symbol-kind heuristic,
//! - [`cursor_decl`] — cursor-on-declaration detection,
//! - [`phpunit`] — the `vendor/bin/phpunit` runner.
//!
//! This module file keeps the LSP file-operation registration, the deferred
//! code-action machinery, and the non-blocking `Backend` wrappers that don't
//! belong to any of the above.

use std::sync::Arc;

use tower_lsp::lsp_types::*;

use crate::document::ast::ParsedDoc;
use crate::navigation::definition::find_declaration_range;

use crate::actions::generate_action::{
    generate_constructor_actions, generate_getters_setters_actions,
};
use crate::actions::implement_action::implement_missing_actions;
use crate::actions::phpdoc_action::phpdoc_actions;
use crate::actions::promote_action::promote_constructor_actions;
use crate::actions::type_action::add_return_type_actions;

use super::Backend;

mod cursor_decl;
mod phpunit;
mod position;

pub(super) use cursor_decl::*;
pub(super) use phpunit::*;
pub(super) use position::*;

pub(super) fn php_file_op() -> FileOperationRegistrationOptions {
    FileOperationRegistrationOptions {
        filters: vec![FileOperationFilter {
            scheme: Some("file".to_string()),
            pattern: FileOperationPattern {
                glob: "**/*.php".to_string(),
                matches: Some(FileOperationPatternKind::File),
                options: None,
            },
        }],
    }
}

/// Strip the `edit` from each `CodeAction` and attach a `data` payload so the
/// client can request the edit lazily via `codeAction/resolve`.
pub(super) fn defer_actions(
    actions: Vec<CodeActionOrCommand>,
    kind_tag: &str,
    uri: &Url,
    range: Range,
) -> Vec<CodeActionOrCommand> {
    actions
        .into_iter()
        .map(|a| match a {
            CodeActionOrCommand::CodeAction(mut ca) => {
                ca.edit = None;
                ca.data = Some(serde_json::json!({
                    "php_lsp_resolve": kind_tag,
                    "uri": uri.to_string(),
                    "range": range,
                }));
                CodeActionOrCommand::CodeAction(ca)
            }
            other => other,
        })
        .collect()
}

/// Tags for deferred code actions (resolved lazily via `codeAction/resolve`).
/// Iteration order controls the order items appear in the client menu.
pub(super) const DEFERRED_ACTION_TAGS: &[&str] = &[
    "phpdoc",
    "implement",
    "constructor",
    "getters_setters",
    "return_type",
    "promote",
];

impl Backend {
    /// Run [`crate::document::document_store::DocumentStore::cached_analysis`] without
    /// blocking the async executor. The warm path (cache entry current for the
    /// file's text) resolves synchronously; the cold path — mir Pass 1 + Pass 2,
    /// which can take hundreds of ms on large files and is hit after every
    /// keystroke because edits clear the analysis cache — runs on the blocking
    /// pool so it doesn't stall other in-flight requests.
    pub(super) async fn cached_analysis_async(
        &self,
        uri: &Url,
    ) -> Option<Arc<mir_analyzer::FileAnalysis>> {
        if let Some(hit) = self.docs.cached_analysis_if_fresh(uri) {
            return Some(hit);
        }
        let docs = Arc::clone(&self.docs);
        let uri_owned = uri.clone();
        match tokio::task::spawn_blocking(move || docs.cached_analysis(&uri_owned)).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("cached_analysis panicked for {uri}: {e}");
                None
            }
        }
    }

    /// Fetch the salsa-memoized workspace aggregate without blocking the async
    /// executor. A warm memo returns quickly, but the cold rebuild after any
    /// file change walks every `FileIndex` in the workspace — run it on the
    /// blocking pool.
    pub(super) async fn workspace_index_async(
        &self,
    ) -> Arc<crate::db::workspace_index::WorkspaceIndexData> {
        let docs = Arc::clone(&self.docs);
        match tokio::task::spawn_blocking(move || docs.get_workspace_index_salsa()).await {
            Ok(wi) => wi,
            // JoinError (panicked/cancelled blocking task): retry inline so a
            // panic surfaces through the caller's panic guard.
            Err(_) => self.docs.get_workspace_index_salsa(),
        }
    }

    /// Reuse `slot` if already populated this request; otherwise fetch and
    /// populate it. Callers MUST reset `slot` to `None` immediately after any
    /// operation that can lazily ingest a new file (`psr4_goto`,
    /// `psr4_method_goto`) so the next fetch sees the fresh file set — this
    /// is a lock-count optimization for the common case where no such
    /// ingestion happens between fallback branches, not an unconditional
    /// cache.
    pub(super) async fn workspace_index_cached(
        &self,
        slot: &mut Option<Arc<crate::db::workspace_index::WorkspaceIndexData>>,
    ) -> Arc<crate::db::workspace_index::WorkspaceIndexData> {
        if let Some(wi) = slot {
            return Arc::clone(wi);
        }
        let wi = self.workspace_index_async().await;
        *slot = Some(Arc::clone(&wi));
        wi
    }

    /// Tag → generator mapping for deferred code actions.
    pub(super) fn generate_deferred_actions(
        &self,
        tag: &str,
        source: &str,
        doc: &Arc<ParsedDoc>,
        range: Range,
        uri: &Url,
    ) -> Vec<CodeActionOrCommand> {
        match tag {
            "phpdoc" => phpdoc_actions(uri, doc, source, range),
            "implement" => {
                let imports = self.file_imports(uri);
                implement_missing_actions(
                    source,
                    doc,
                    &self.docs.all_docs_for_scan(),
                    range,
                    uri,
                    &imports,
                )
            }
            "constructor" => generate_constructor_actions(source, doc, range, uri),
            "getters_setters" => generate_getters_setters_actions(source, doc, range, uri),
            "return_type" => add_return_type_actions(source, doc, range, uri),
            "promote" => promote_constructor_actions(source, doc, range, uri),
            _ => Vec::new(),
        }
    }

    /// Try to resolve a fully-qualified name via the PSR-4 map, with PSR-0 fallback.
    /// Indexes the file on-demand if it is not already in the document store.
    pub(super) async fn psr4_goto(&self, fqn: &str) -> Option<Location> {
        let psr4 = self.psr4.load();
        let path = psr4.resolve(fqn).or_else(|| psr4.psr0_resolve(fqn))?;

        let file_uri = Url::from_file_path(&path).ok()?;

        // Index on-demand if the file was not picked up by the workspace scan.
        // Use `get_doc_salsa_any` (ignores open-file gating): after `ingest()`
        // the file is mirrored but background-only, and the call site needs
        // the AST regardless of whether the editor has the file open.
        if self.docs.get_doc_salsa(&file_uri).is_none() {
            let text = tokio::fs::read_to_string(&path).await.ok()?;
            self.ingest_if_not_open(file_uri.clone(), &text);
        }

        let doc = self.docs.get_doc_salsa(&file_uri)?;

        // Classes are declared by their short (unqualified) name, e.g. `class Foo`
        // not `class App\Services\Foo`.
        let short_name = fqn.split('\\').next_back()?;
        let range = find_declaration_range(doc.source(), &doc, short_name)?;

        Some(Location {
            uri: file_uri,
            range,
        })
    }

    /// Walk the PSR-4 class hierarchy starting from `class_fqn` to find the
    /// definition of `method_name`. Follows the PHP method-resolution order
    /// (traits → parent) through vendor files that were excluded from the
    /// eager workspace scan. Files are lazily ingested into the document store
    /// on first visit; their `FileIndex` is cached in `vendor_index_cache` so
    /// repeated navigation to the same vendor class is cheap.
    pub(super) async fn psr4_method_goto(
        &self,
        class_fqn: &str,
        method_name: &str,
    ) -> Option<Location> {
        use crate::index::file_index::FileIndex;
        use crate::navigation::definition::{find_declaration_range, find_method_range_in_class};
        use crate::text::zero_width_range;
        use std::collections::{HashSet, VecDeque};

        let mut queue: VecDeque<String> = VecDeque::from([class_fqn.to_owned()]);
        let mut visited: HashSet<String> = HashSet::new();

        while let Some(fqn) = queue.pop_front() {
            if !visited.insert(fqn.clone()) {
                continue;
            }

            let path = match self.psr4.load().resolve(&fqn) {
                Some(p) => p,
                None => continue,
            };
            let uri = match Url::from_file_path(&path) {
                Ok(u) => u,
                Err(_) => continue,
            };

            // Lazy-load into the workspace so get_doc_salsa works below.
            if self.docs.get_doc_salsa(&uri).is_none() {
                let text = match tokio::fs::read_to_string(&path).await {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                self.ingest_if_not_open(uri.clone(), &text);
            }

            let doc = match self.docs.get_doc_salsa(&uri) {
                Some(d) => d,
                None => continue,
            };

            // Use a cached FileIndex when available to avoid re-extracting.
            let index = self.docs.get_vendor_index(&uri).unwrap_or_else(|| {
                let idx = Arc::new(FileIndex::extract(&doc));
                self.docs.cache_vendor_index(uri.clone(), Arc::clone(&idx));
                idx
            });

            let short = crate::text::fqn_short_name(&fqn);

            for cls in &index.classes {
                if cls.name.as_ref() != short {
                    continue;
                }

                for m in &cls.methods {
                    if m.name.as_ref() == method_name {
                        let range = find_method_range_in_class(&doc, short, method_name)
                            .or_else(|| find_declaration_range(doc.source(), &doc, method_name))
                            .unwrap_or_else(|| zero_width_range(m.start_line));
                        return Some(Location { uri, range });
                    }
                }
                for dm in &cls.doc_methods {
                    if dm.name.as_ref() == method_name {
                        return Some(Location {
                            uri,
                            range: zero_width_range(dm.start_line),
                        });
                    }
                }

                // Queue parent chain in PHP MRO order: traits → mixins → parent.
                for trt in &cls.traits {
                    queue.push_back(resolve_name_to_fqn(trt.as_ref(), &index));
                }
                for mx in &cls.mixins {
                    queue.push_back(resolve_name_to_fqn(mx.as_ref(), &index));
                }
                if let Some(parent) = &cls.parent {
                    queue.push_back(resolve_name_to_fqn(parent.as_ref(), &index));
                }
            }
        }
        None
    }

    /// Pre-load via PSR-4 any direct supertypes of `item_name` that are not yet
    /// present in the workspace index, so the next call to `workspace_index_async`
    /// will include them. Only one level is loaded (direct parents / interfaces);
    /// the type-hierarchy feature only ever requests one level at a time.
    /// Returns `true` when at least one new file was ingested.
    pub(super) async fn ensure_direct_supertypes_loaded(
        &self,
        item_name: &str,
        wi: &crate::db::workspace_index::WorkspaceIndexData,
    ) -> bool {
        let refs = match wi.classes_by_name.get(item_name) {
            Some(r) => r.clone(),
            None => return false,
        };

        let mut ingested = false;
        for r in &refs {
            let Some((_, cls)) = wi.at(*r) else {
                continue;
            };
            let file_idx = wi.files.get(r.file as usize).map(|(_, idx)| idx.as_ref());

            let mut super_names: Vec<String> = Vec::new();
            if let Some(p) = &cls.parent {
                super_names.push(p.as_ref().to_owned());
            }
            for iface in &cls.implements {
                super_names.push(iface.as_ref().to_owned());
            }

            for name in super_names {
                let short = crate::text::fqn_short_name(&name);
                if wi.classes_by_name.contains_key(short) {
                    continue;
                }
                // Resolve short name to FQN via the implementing file's use_imports.
                let fqn = if let Some(idx) = file_idx {
                    resolve_name_to_fqn(&name, idx)
                } else {
                    name.clone()
                };
                let path = match self.psr4.load().resolve(&fqn) {
                    Some(p) => p,
                    None => continue,
                };
                let uri = match Url::from_file_path(&path) {
                    Ok(u) => u,
                    Err(_) => continue,
                };
                if self.docs.get_doc_salsa(&uri).is_some() {
                    continue;
                }
                let text = match tokio::fs::read_to_string(&path).await {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                self.ingest_if_not_open(uri, &text);
                ingested = true;
            }
        }
        ingested
    }

}

/// Resolve a potentially-short class `name` to a fully-qualified name by
/// looking it up in `index.use_imports` and `index.namespace`. Used when
/// walking a vendor class hierarchy where parent names are stored as written
/// in the source (e.g. `"AbstractController"` rather than the full FQN).
fn resolve_name_to_fqn(name: &str, index: &crate::index::file_index::FileIndex) -> String {
    // Already qualified — strip leading backslash and return.
    if name.contains('\\') {
        return name.trim_start_matches('\\').to_owned();
    }
    // Resolve through `use` imports (e.g. `use Symfony\...\AbstractController`).
    for (alias, fqn) in &index.use_imports {
        if alias.as_ref() == name {
            return fqn.as_ref().trim_start_matches('\\').to_owned();
        }
    }
    // Apply the current namespace as the last resort.
    if let Some(ns) = &index.namespace {
        return format!("{}\\{}", ns.trim_start_matches('\\'), name);
    }
    name.to_owned()
}
