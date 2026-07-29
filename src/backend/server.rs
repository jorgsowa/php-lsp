use std::sync::Arc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{LanguageServer, async_trait};

use super::panic_guard::{guard_async, guard_async_result};
use crate::completion::{CompletionCtx, filtered_completions_at};
use crate::document::ast::ParsedDoc;
use crate::document::open_files::compute_open_file_diagnostics;
use crate::hover::{
    class_hover_from_index, docs_for_symbol_from_index, docs_for_symbol_from_index_scoped,
    extract_static_class_before_cursor, hover_info_with_maps, method_hover_from_index,
    signature_for_symbol_from_index_scoped,
};
use crate::index::file_index::ClassKind;
use crate::index::workspace_scan::{scan_workspace, send_refresh_requests};
use crate::lang::config::LspConfig;
use crate::navigation::symbols::{
    document_symbols, resolve_workspace_symbol, workspace_symbols_from_workspace,
};
use crate::text::word_at_position;

use crate::navigation::call_hierarchy::{
    incoming_calls_indexed, outgoing_calls_indexed, prepare_call_hierarchy_indexed,
};
use crate::navigation::declaration::{goto_declaration, goto_declaration_from_index};
use crate::navigation::moniker::moniker_at;
use crate::navigation::type_definition::{
    goto_type_definition_exact, goto_type_definition_from_index_exact,
    goto_type_definition_from_index_short_name_fallback, goto_type_definition_short_name_fallback,
};
use crate::navigation::type_hierarchy::{
    prepare_type_hierarchy_from_workspace, subtypes_of_mir_backed, supertypes_of_from_workspace,
};

use crate::analysis::code_lens::code_lenses;
use crate::analysis::diagnostics::{diagnostics_from_doc, parse_document, parse_document_no_diags};
use crate::analysis::document_highlight::document_highlights;
use crate::analysis::inlay_hints::inlay_hints;
use crate::analysis::inline_value::inline_values_in_range;
use crate::analysis::semantic_tokens::{
    compute_token_delta, semantic_tokens, semantic_tokens_range, token_hash,
};

use crate::editing::document_link::document_links;
use crate::editing::folding::folding_ranges;
use crate::editing::formatting::{format_document, format_range};
use crate::editing::on_type_format::on_type_format;
use crate::editing::rename::{prepare_rename, rename_variable};
use crate::editing::selection_range::selection_ranges;
use crate::editing::signature_help::signature_help;

use super::helpers::{
    cursor_is_on_method_decl, cursor_is_on_property_decl, promoted_property_at_cursor, run_phpunit,
};
use super::{Backend, publish_with_dependents};

/// Idle time after the last edit before the background analysis re-warm runs.
/// Long enough that a typing burst triggers one sweep, not one per pause;
/// short enough that references asked "shortly after editing" are warm again.
const WARM_RESWEEP_IDLE_MS: u64 = 1_000;

#[async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.handle_initialize(params).await
    }

    async fn initialized(&self, _params: InitializedParams) {
        guard_async("initialized", async move {
            self.handle_initialized(_params).await
        })
        .await
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        guard_async("did_change_configuration", async move {
            // Pull the current configuration from the client rather than parsing the
            // (often-null) params.settings, which not all clients populate.
            let items = vec![ConfigurationItem {
                scope_uri: None,
                section: Some("php-lsp".to_string()),
            }];
            if let Ok(values) = self.client.configuration(items).await
                && let Some(value) = values.into_iter().next()
            {
                let roots = self.root_paths.load_full();

                // Re-read .php-lsp.json so a user who edits the file and then
                // triggers a configuration reload picks up the latest values.
                let file_cfg = crate::lang::autoload::load_project_config_json(&roots);

                if let Some(ver) = value.get("phpVersion").and_then(|v| v.as_str())
                    && !crate::lang::autoload::is_valid_php_version(ver)
                {
                    self.client
                        .log_message(
                            tower_lsp::lsp_types::MessageType::WARNING,
                            format!(
                                "php-lsp: unsupported phpVersion {ver:?} — valid values: {}",
                                crate::lang::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                            ),
                        )
                        .await;
                }

                let file_obj = file_cfg.as_ref().filter(|v| v.is_object());
                let merged = LspConfig::merge_project_configs(file_obj, Some(&value));
                let mut cfg = LspConfig::from_value(&merged);

                // Resolve the PHP version and log what was chosen and why.
                let (ver, source) = self.resolve_php_version(cfg.php_version.as_deref());
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::INFO,
                        format!("php-lsp: using PHP {ver} ({source})"),
                    )
                    .await;
                // Clamp unsupported versions to the nearest supported one and warn.
                let ver = if source != "set by editor"
                    && !crate::lang::autoload::is_valid_php_version(&ver)
                {
                    let clamped = crate::lang::autoload::clamp_php_version(&ver);
                    self.client
                        .show_message(
                            tower_lsp::lsp_types::MessageType::WARNING,
                            format!(
                                "php-lsp: detected PHP {ver} is outside the supported range ({}); \
                             using PHP {clamped} for analysis",
                                crate::lang::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                            ),
                        )
                        .await;
                    clamped.to_string()
                } else {
                    ver
                };
                cfg.php_version = Some(ver.clone());
                if let Ok(pv) = ver.parse::<mir_analyzer::PhpVersion>() {
                    self.docs.set_php_version(pv);
                }
                self.config.store(Arc::new(cfg));
                send_refresh_requests(&self.client).await;
            }
        })
        .await
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        guard_async("did_change_workspace_folders", async move {
            // Remove folders from our tracked roots.
            {
                let mut roots = (**self.root_paths.load()).clone();
                for removed in &params.event.removed {
                    if let Ok(path) = removed.uri.to_file_path() {
                        roots.retain(|r| r != &path);
                    }
                }
                self.root_paths.store(Arc::new(roots));
            }

            // Add new folders and kick off background scans for each.
            let (exclude_paths, include_paths, max_indexed_files, cache_path) = {
                let cfg = self.config.load();
                (
                    cfg.exclude_paths.clone(),
                    cfg.include_paths.clone(),
                    cfg.max_indexed_files,
                    cfg.cache_path.clone(),
                )
            };
            for added in &params.event.added {
                if let Ok(path) = added.uri.to_file_path() {
                    let is_new = {
                        let mut roots = (**self.root_paths.load()).clone();
                        if !roots.contains(&path) {
                            roots.push(path.clone());
                            self.root_paths.store(Arc::new(roots));
                            true
                        } else {
                            false
                        }
                    };
                    if is_new {
                        let docs = Arc::clone(&self.docs);
                        let open_files = self.open_files.clone();
                        let ex = exclude_paths.clone();
                        let ip = include_paths.clone();
                        let path_clone = path.clone();
                        let client = self.client.clone();
                        let cp = cache_path.clone();
                        tokio::spawn(async move {
                            let cache = if let Some(p) = cp {
                                Some(crate::index::cache::WorkspaceCache::with_dir(p))
                            } else {
                                crate::index::cache::WorkspaceCache::new(&path_clone)
                            };
                            scan_workspace(
                                path_clone,
                                docs,
                                open_files,
                                cache,
                                &ex,
                                &ip,
                                max_indexed_files,
                                None,
                            )
                            .await;
                            send_refresh_requests(&client).await;
                        });
                    }
                }
            }
        })
        .await
    }

    async fn shutdown(&self) -> Result<()> {
        // Postings committed since the last warm sweep (on-demand query
        // freshness passes, edit re-sweeps) reach disk only via a flush.
        let docs = Arc::clone(&self.docs);
        let _ = tokio::task::spawn_blocking(move || docs.flush_analysis_cache()).await;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        guard_async("did_open", async move {
            let uri = params.text_document.uri;
            let text = params.text_document.text;

            // Store text immediately so other features work while parsing.
            // This also mirrors the new text into salsa, so the codebase query
            // sees it when semantic_diagnostics runs below.
            self.set_open_text(uri.clone(), text);

            // Seed parse diagnostics from the salsa-cached doc. On the fast
            // path this is a lock-free DashMap lookup — no re-parse.
            let parse_diags = self
                .docs
                .get_doc_salsa(&uri)
                .map(|doc| diagnostics_from_doc(&doc))
                .unwrap_or_default();
            self.set_parse_diagnostics(&uri, parse_diags);

            publish_with_dependents(
                self.client.clone(),
                Arc::clone(&self.docs),
                self.open_files.clone(),
                uri,
                self.config.load().diagnostics.clone(),
            )
            .await;
        })
        .await
    }

    #[tracing::instrument(skip_all)]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        guard_async("did_change", async move {
            let uri = params.text_document.uri;
            // Incremental sync: apply changes in order to the live buffer.
            // Each ranged change refers to the document state produced by the
            // previous one; a change without a range is a full-document
            // replacement (clients may still send those under INCREMENTAL).
            let mut updated: Option<String> = None;
            for change in params.content_changes {
                match change.range {
                    None => updated = Some(change.text),
                    Some(range) => {
                        let mut cur = match updated.take() {
                            Some(t) => t,
                            None => self.get_open_text(&uri).unwrap_or_default(),
                        };
                        crate::text::apply_content_change(&mut cur, range, &change.text);
                        updated = Some(cur);
                    }
                }
            }
            let Some(text) = updated else { return };

            // Store text immediately and capture the version token.
            // Features (completion, hover, …) see the new text instantly while
            // the parse runs in the background.
            let version = self.set_open_text(uri.clone(), text.clone());

            let docs = Arc::clone(&self.docs);
            let open_files = self.open_files.clone();
            let client = self.client.clone();
            let cfg = self.config.load();
            let diag_cfg = cfg.diagnostics.clone();
            let debounce_ms = cfg.debounce_ms;
            let warm_analysis = cfg.warm_analysis;
            tokio::spawn(async move {
                // Debounce: if another edit arrives before we parse, the version
                // gate below will discard this result.
                tokio::time::sleep(std::time::Duration::from_millis(debounce_ms)).await;

                // Skip the expensive parse+analyze if a newer edit already
                // superseded this one. Collapses N rapid keystrokes into 1
                // spawn_blocking call instead of N.
                if open_files.current_version(&uri) != Some(version) {
                    return;
                }

                let (_doc, parse_diags) =
                    tokio::task::spawn_blocking(move || parse_document(&text))
                        .await
                        .unwrap_or_else(|e| {
                            tracing::warn!("parse_document panicked for {uri}: {e}");
                            (ParsedDoc::default(), vec![])
                        });

                // Only apply if no newer edit arrived while we were parsing.
                if open_files.current_version(&uri) != Some(version) {
                    return;
                }
                open_files.set_parse_diagnostics(&uri, parse_diags);
                publish_with_dependents(
                    client,
                    Arc::clone(&docs),
                    open_files.clone(),
                    uri.clone(),
                    diag_cfg,
                )
                .await;

                // Re-warm the analysis memos this edit invalidated once typing
                // goes idle, so the next references/rename stays a memo hit.
                // Unaffected files revalidate without re-analysis, so the sweep
                // cost tracks what the edit actually touched.
                if warm_analysis && docs.is_index_ready() {
                    tokio::time::sleep(std::time::Duration::from_millis(WARM_RESWEEP_IDLE_MS))
                        .await;
                    if open_files.current_version(&uri) != Some(version) {
                        return;
                    }
                    let open_urls = open_files.urls();
                    drop(tokio::task::spawn_blocking(move || {
                        let cancel = docs.begin_warm_sweep();
                        docs.warm_analysis_sweep(&open_urls, &cancel);
                    }));
                }
            });
        })
        .await
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        guard_async("did_close", async move {
            let uri = params.text_document.uri;
            self.close_open_file(&uri);
            // The salsa-mirrored text still holds the last edited buffer, which may
            // include unsaved changes the user just discarded on close. Re-sync from
            // disk so cross-file features resolve against the on-disk content; if the
            // file is gone (never saved / deleted), drop it from the index.
            let disk_text = match uri.to_file_path() {
                Ok(path) => tokio::fs::read_to_string(&path).await.ok(),
                Err(_) => None,
            };
            // The disk read awaited above; a reopen may have raced in. Only mutate
            // the index if the file is still closed, so we never clobber a fresh
            // buffer.
            if !self.open_files.contains(&uri) {
                match disk_text {
                    Some(text) => self.docs.ingest(uri.clone(), &text),
                    None => self.docs.remove(&uri),
                }
            }
            // Clear editor diagnostics; the file stays indexed for cross-file features
            self.client.publish_diagnostics(uri, vec![], None).await;
        })
        .await
    }

    async fn will_save(&self, _params: WillSaveTextDocumentParams) {}

    async fn will_save_wait_until(
        &self,
        params: WillSaveTextDocumentParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        guard_async_result("will_save_wait_until", async move {
            let source = self
                .get_open_text(&params.text_document.uri)
                .unwrap_or_default();
            Ok(format_document(&source))
        })
        .await
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        guard_async("did_save", async move {
            let uri = params.text_document.uri;
            // Re-publish diagnostics on save so editors that defer diagnostics
            // until save (rather than on every keystroke) see up-to-date results.
            // Must include semantic diagnostics — publishDiagnostics replaces the
            // prior set entirely, so omitting them would clear errors the editor
            // showed after the last did_change.
            let diag_cfg = self.config.load().diagnostics.clone();
            let all = compute_open_file_diagnostics(&self.docs, &self.open_files, &uri, &diag_cfg);
            self.open_files
                .note_published(&uri, super::diagnostics_content_hash(&all));
            self.client
                .publish_diagnostics(uri.clone(), all, None)
                .await;

            // Persist the FileIndex to the disk cache so that a server restart
            // can skip re-parsing this file even for edits that happened between
            // workspace scans. Content-keyed so the entry matches the scan's key
            // on restart regardless of mtime granularity.
            let cache_path = self.config.load().cache_path.clone();
            if let Ok(path) = uri.to_file_path() {
                let root = self.root_paths.load().first().cloned();
                tokio::task::spawn_blocking(move || {
                    let cache = if let Some(p) = cache_path {
                        crate::index::cache::WorkspaceCache::with_dir(p)
                    } else if let Some(r) = root {
                        let Some(c) = crate::index::cache::WorkspaceCache::new(&r) else {
                            return;
                        };
                        c
                    } else {
                        return;
                    };
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        return;
                    };
                    let key = crate::index::cache::WorkspaceCache::key_for(uri.as_str(), &text);
                    let doc = parse_document_no_diags(&text);
                    let index = crate::index::file_index::FileIndex::extract(&doc);
                    let _ = cache.write(&key, &index);
                });
            }
        })
        .await
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        guard_async("did_change_watched_files", async move {
            for change in params.changes {
                match change.typ {
                    FileChangeType::CREATED | FileChangeType::CHANGED => {
                        if let Ok(path) = change.uri.to_file_path()
                            && let Ok(text) = tokio::fs::read_to_string(&path).await
                        {
                            // Salsa path: ingest_from_doc mirrors the new text into
                            // the SourceFile input. On the next codebase() call,
                            // salsa re-runs file_definitions for this file and the
                            // aggregator re-folds — no manual remove/collect/finalize.
                            let doc = parse_document_no_diags(&text);
                            self.ingest_from_doc_if_not_open(change.uri.clone(), &doc);
                        }
                    }
                    FileChangeType::DELETED => {
                        self.docs.remove(&change.uri);
                    }
                    _ => {}
                }
            }
            // File changes may affect cross-file features — refresh all live editors.
            send_refresh_requests(&self.client).await;

            // Bulk external changes (git checkout, generators) invalidate many
            // analysis memos at once; re-warm so references stay fast after a
            // branch switch, not just after in-editor edits.
            if self.config.load().warm_analysis && self.docs.is_index_ready() {
                let docs = Arc::clone(&self.docs);
                let open_urls = self.open_files.urls();
                drop(tokio::task::spawn_blocking(move || {
                    let cancel = docs.begin_warm_sweep();
                    docs.warm_analysis_sweep(&open_urls, &cancel);
                }));
            }
        })
        .await
    }

    #[tracing::instrument(skip_all)]
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        guard_async_result("completion", async move {
            let uri = &params.text_document_position.text_document.uri;
            let position = params.text_document_position.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(Some(CompletionResponse::Array(vec![]))),
            };
            let other_docs: Vec<Arc<ParsedDoc>> = self
                .docs
                .other_docs(uri, &self.open_urls())
                .into_iter()
                .map(|(_, d)| d)
                .collect();
            // Clone to owned so the closure is 'static + Send.
            let trigger = params
                .context
                .as_ref()
                .and_then(|c| c.trigger_character.clone());
            let laravel_arc = self.laravel.load_full();
            let imports = self.file_imports(uri);
            let wi = self.workspace_index_async().await;
            let wi_for_class_search = Arc::clone(&wi);
            let uri_for_class_search = uri.clone();
            let docs_for_lookup = Arc::clone(&self.docs);
            let find_class_doc_fn = move |name: &str| -> Option<Arc<ParsedDoc>> {
                let cr = wi.resolve_class_ref(name)?;
                let (uri, _) = wi.at(cr)?;
                docs_for_lookup.get_doc_salsa(uri)
            };
            // Finds classes anywhere in the workspace index — including
            // vendor/ and other files that are never opened as editor
            // buffers — whose short name starts with `prefix`. Complements
            // `other_docs`, which only covers currently open documents.
            //
            // `classes_by_lowercase_name` is sorted once per workspace
            // revision (shared via `Arc` across every completion request
            // until a file changes), so this does a binary search for the
            // prefix range instead of scanning + re-lowercasing every class
            // name on every keystroke.
            const WORKSPACE_CLASS_SEARCH_LIMIT: usize = 50;
            let workspace_class_search_fn =
                move |prefix: &str| -> Vec<(String, CompletionItemKind, String)> {
                    let prefix_lc = prefix.to_lowercase();
                    let table = &wi_for_class_search.classes_by_lowercase_name;
                    let start =
                        table.partition_point(|(name, _)| name.as_ref() < prefix_lc.as_str());
                    let mut out = Vec::new();
                    for (name, cr) in &table[start..] {
                        if !name.starts_with(prefix_lc.as_str()) {
                            break;
                        }
                        let Some((file_uri, cls)) = wi_for_class_search.at(*cr) else {
                            continue;
                        };
                        if *file_uri == uri_for_class_search {
                            continue;
                        }
                        let kind = match cls.kind {
                            ClassKind::Class | ClassKind::Trait => CompletionItemKind::CLASS,
                            ClassKind::Interface => CompletionItemKind::INTERFACE,
                            ClassKind::Enum => CompletionItemKind::ENUM,
                        };
                        out.push((cls.name.to_string(), kind, cls.fqn.to_string()));
                        if out.len() >= WORKSPACE_CLASS_SEARCH_LIMIT {
                            break;
                        }
                    }
                    out
                };
            let analysis = self.cached_analysis_async(uri).await;
            let session = self
                .docs
                .analysis_session(self.docs.workspace_php_version());
            let uri_owned = uri.clone();
            let uri_str = uri.to_string();
            // Offload to spawn_blocking: filtered_completions_at walks the full
            // AST + workspace index which can take tens of milliseconds on large
            // files, blocking the async executor from unrelated requests.
            let items = match tokio::task::spawn_blocking(move || {
                let ctx = CompletionCtx {
                    source: Some(&source),
                    position: Some(position),
                    doc_uri: Some(&uri_owned),
                    file_imports: Some(&imports),
                    find_class_doc: Some(&find_class_doc_fn),
                    workspace_class_search: Some(&workspace_class_search_fn),
                    analysis: analysis.as_deref(),
                    session: Some(session),
                    laravel: Some(&laravel_arc),
                };
                filtered_completions_at(&doc, &other_docs, trigger.as_deref(), &ctx)
            })
            .await
            {
                Ok(items) => items,
                Err(e) => {
                    tracing::warn!("completion panicked for {uri_str}: {e}");
                    vec![]
                }
            };
            Ok(Some(CompletionResponse::Array(items)))
        })
        .await
    }

    async fn completion_resolve(&self, mut item: CompletionItem) -> Result<CompletionItem> {
        guard_async_result("completion_resolve", async move {
            if item.documentation.is_some() && item.detail.is_some() {
                return Ok(item);
            }
            // Strip trailing ':' from named-argument labels (e.g. "param:") before lookup.
            let name = item.label.trim_end_matches(':');
            // Method completion items carry their owning class in `data` (see
            // member.rs's all_members) so two unrelated classes declaring a
            // same-named method don't resolve to whichever is indexed first.
            let class_hint = item
                .data
                .as_ref()
                .and_then(|d| d.get("class"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            self.docs.with_all_indexes(|all_indexes| {
                if item.detail.is_none()
                    && let Some(sig) =
                        signature_for_symbol_from_index_scoped(name, all_indexes, class_hint.as_deref())
                {
                    item.detail = Some(sig);
                }
                if item.documentation.is_none()
                    && let Some(md) =
                        docs_for_symbol_from_index_scoped(name, all_indexes, class_hint.as_deref())
                {
                    item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: md,
                    }));
                }
            });
            Ok(item)
        })
        .await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        guard_async_result("goto_definition", async move {
            self.handle_goto_definition(params).await
        })
        .await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        guard_async_result(
            "references",
            async move { self.handle_references(params).await },
        )
        .await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        guard_async_result("prepare_rename", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            Ok(prepare_rename(&doc, params.position).map(PrepareRenameResponse::Range))
        })
        .await
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        guard_async_result("rename", async move {
            let uri = &params.text_document_position.text_document.uri;
            let position = params.text_document_position.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let word = match word_at_position(&source, position) {
                Some(w) => w,
                None => return Ok(None),
            };
            if word.starts_with('$') {
                let doc = match self.get_doc(uri) {
                    Some(d) => d,
                    None => return Ok(None),
                };
                // Cursor on a property declaration (`public int $x`) or a promoted
                // constructor parameter (`private string $x`) — both act as property
                // declarations and take the cross-file indexed path below. Plain
                // variables stay on the single-document scope walker.
                let on_property_decl =
                    cursor_is_on_property_decl(&source, &doc.program().stmts, position).is_some()
                        || promoted_property_at_cursor(&source, &doc.program().stmts, position)
                            .is_some();
                if !on_property_decl {
                    return Ok(Some(rename_variable(
                        &word,
                        &params.new_name,
                        uri,
                        &doc,
                        position,
                    )));
                }
            }
            // Everything else — classes, functions, methods, properties,
            // constants — renames from mir's posting lists (declaration
            // token and `use` import lines included).
            Ok(self.indexed_rename(uri, position, &params.new_name).await)
        })
        .await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        guard_async_result("signature_help", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let index_data = self.docs.get_workspace_index_salsa();
            let analysis = self.cached_analysis_async(uri).await;
            let uri_str = uri.to_string();
            let doc_clone = Arc::clone(&doc);
            let result = match tokio::task::spawn_blocking(move || {
                signature_help(
                    &source,
                    &doc_clone,
                    position,
                    &index_data.files,
                    analysis.as_deref(),
                )
            })
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("signature_help panicked for {uri_str}: {e}");
                    None
                }
            };
            Ok(result)
        })
        .await
    }

    #[tracing::instrument(skip_all)]
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        guard_async_result("hover", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let other_docs = self.docs.other_docs(uri, &self.open_urls());
            let other_maps = self.docs.other_symbol_maps(uri, &self.open_urls());
            let analysis = self.cached_analysis_async(uri).await;
            let hover_session = self
                .docs
                .analysis_session(self.docs.workspace_php_version());
            let source_clone = source.clone();
            let doc_clone = Arc::clone(&doc);
            let uri_str = uri.to_string();
            // Lets mir-member/static-prop hover resolve a class's declaring doc
            // via the workspace index even when that file isn't open in the
            // editor, instead of only searching `other_docs` (open files).
            let wi = self.workspace_index_async().await;
            let docs_for_lookup = Arc::clone(&self.docs);
            let find_class_doc_fn = move |name: &str| -> Option<Arc<ParsedDoc>> {
                let cr = wi.resolve_class_ref(name)?;
                let (uri, _) = wi.at(cr)?;
                docs_for_lookup.get_doc_salsa(uri)
            };
            let result = match tokio::task::spawn_blocking(move || {
                hover_info_with_maps(
                    &source_clone,
                    &doc_clone,
                    analysis.as_deref(),
                    position,
                    &other_docs,
                    &other_maps,
                    Some(&hover_session),
                    Some(&find_class_doc_fn),
                )
            })
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("hover panicked for {uri_str}: {e}");
                    None
                }
            };
            if result.is_some() {
                return Ok(result);
            }
            // Fallback: look up the word in the workspace index so class names in
            // extends clauses and parameter types resolve even when their defining
            // file is never opened.  Also try the alias-resolved name so that
            // `use Foo as Bar` works even when Foo is only in the index.
            if let Some(word) = crate::text::word_at_position(&source, position) {
                let wi = self.workspace_index_async().await;
                // Try the literal word first.
                if let Some(h) = class_hover_from_index(&word, None, &wi.files) {
                    return Ok(Some(h));
                }
                // Try alias resolution. The resolved FQN disambiguates between
                // same-named classes in different namespaces (e.g. many
                // vendored `Factory` classes all aliased to `FactoryContract`).
                if let Some((resolved, resolved_fqn)) =
                    crate::hover::resolve_use_alias_fqn(&doc.program().stmts, &word)
                    && let Some(h) =
                        class_hover_from_index(&resolved, Some(&resolved_fqn), &wi.files)
                {
                    return Ok(Some(h));
                }
                // Try static method hover: `ClassName::method(…)`.
                if let Some(line_text) = source.lines().nth(position.line as usize)
                    && let Some(class_token) =
                        extract_static_class_before_cursor(line_text, position.character as usize)
                {
                    if let Some(h) = method_hover_from_index(&class_token, &word, &wi.files) {
                        return Ok(Some(h));
                    }
                    if let Some(resolved_class) =
                        crate::hover::resolve_use_alias(&doc.program().stmts, &class_token)
                        && let Some(h) = method_hover_from_index(&resolved_class, &word, &wi.files)
                    {
                        return Ok(Some(h));
                    }
                }
            }
            Ok(None)
        })
        .await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        guard_async_result("document_symbol", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            Ok(Some(DocumentSymbolResponse::Nested(document_symbols(
                doc.source(),
                &doc,
            ))))
        })
        .await
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        guard_async_result("folding_range", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let ranges = folding_ranges(doc.source(), &doc);
            Ok(if ranges.is_empty() {
                None
            } else {
                Some(ranges)
            })
        })
        .await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        guard_async_result("inlay_hint", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let analysis = self.cached_analysis_async(uri).await;
            let wi = self.workspace_index_async().await;
            let uri_str = uri.to_string();
            let hints = match tokio::task::spawn_blocking(move || {
                inlay_hints(
                    doc.source(),
                    &doc,
                    analysis.as_deref(),
                    params.range,
                    &wi.files,
                )
            })
            .await
            {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!("inlay_hint panicked for {uri_str}: {e}");
                    Vec::new()
                }
            };
            Ok(Some(hints))
        })
        .await
    }

    async fn inlay_hint_resolve(&self, mut item: InlayHint) -> Result<InlayHint> {
        let fallback = item.clone();
        let resolved = guard_async("inlay_hint_resolve", async move {
            if item.tooltip.is_some() {
                return Some(item);
            }
            let func_name = item
                .data
                .as_ref()
                .and_then(|d| d.get("php_lsp_fn"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if let Some(name) = func_name {
                self.docs.with_all_indexes(|all_indexes| {
                    if let Some(md) = docs_for_symbol_from_index(&name, all_indexes) {
                        item.tooltip = Some(InlayHintTooltip::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: md,
                        }));
                    }
                });
            }
            Some(item)
        })
        .await;
        Ok(resolved.unwrap_or(fallback))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        guard_async_result("symbol", async move {
            // Phase J: read through the salsa-memoized aggregate so repeated
            // workspace-symbol queries (every keystroke in the picker) share the
            // same `Arc` until a file changes.
            let wi = self.workspace_index_async().await;
            let query = params.query;
            let results = match tokio::task::spawn_blocking(move || {
                workspace_symbols_from_workspace(&query, &wi)
            })
            .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("workspace/symbol panicked: {e}");
                    Vec::new()
                }
            };
            Ok(Some(results))
        })
        .await
    }

    async fn symbol_resolve(&self, params: WorkspaceSymbol) -> Result<WorkspaceSymbol> {
        // For resolve, we need the full range from the ParsedDoc of open files.
        let docs = self.docs.docs_for(&self.open_urls());
        let fallback = params.clone();
        let resolved = guard_async("symbol_resolve", async move {
            Some(resolve_workspace_symbol(params, &docs))
        })
        .await;
        Ok(resolved.unwrap_or(fallback))
    }

    #[tracing::instrument(skip_all)]
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        guard_async_result("semantic_tokens_full", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => {
                    return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                        result_id: None,
                        data: vec![],
                    })));
                }
            };
            let tokens = semantic_tokens(doc.source(), &doc);
            let result_id = token_hash(&tokens);
            let tokens_arc = Arc::new(tokens);
            self.docs
                .store_token_cache(uri, result_id.clone(), Arc::clone(&tokens_arc));
            let data = Arc::try_unwrap(tokens_arc).unwrap_or_else(|arc| (*arc).clone());
            Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: Some(result_id),
                data,
            })))
        })
        .await
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>> {
        guard_async_result("semantic_tokens_range", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => {
                    return Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
                        result_id: None,
                        data: vec![],
                    })));
                }
            };
            let tokens = semantic_tokens_range(doc.source(), &doc, params.range);
            Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
                result_id: None,
                data: tokens,
            })))
        })
        .await
    }

    async fn semantic_tokens_full_delta(
        &self,
        params: SemanticTokensDeltaParams,
    ) -> Result<Option<SemanticTokensFullDeltaResult>> {
        guard_async_result("semantic_tokens_full_delta", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };

            let new_tokens = Arc::new(semantic_tokens(doc.source(), &doc));
            let new_result_id = token_hash(&new_tokens);
            let prev_id = &params.previous_result_id;

            let result = match self.docs.get_token_cache(uri, prev_id) {
                Some(old_tokens) => {
                    let edits = compute_token_delta(&old_tokens, &new_tokens);
                    SemanticTokensFullDeltaResult::TokensDelta(SemanticTokensDelta {
                        result_id: Some(new_result_id.clone()),
                        edits,
                    })
                }
                // Unknown previous result — fall back to full tokens
                None => SemanticTokensFullDeltaResult::Tokens(SemanticTokens {
                    result_id: Some(new_result_id.clone()),
                    data: (*new_tokens).clone(),
                }),
            };

            self.docs.store_token_cache(uri, new_result_id, new_tokens);
            Ok(Some(result))
        })
        .await
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        guard_async_result("selection_range", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let ranges = selection_ranges(&doc, &params.positions);
            Ok(if ranges.is_empty() {
                None
            } else {
                Some(ranges)
            })
        })
        .await
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        guard_async_result("prepare_call_hierarchy", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let word = match word_at_position(&source, position) {
                Some(w) => w,
                None => return Ok(None),
            };
            // O(matches) lookup via the aggregate's `decls_by_name` map instead
            // of scanning every workspace doc.
            let wi = self.workspace_index_async().await;
            let docs = Arc::clone(&self.docs);
            let get_doc = move |u: &Url| docs.get_doc_salsa(u);
            Ok(prepare_call_hierarchy_indexed(&word, &wi, &get_doc).map(|item| vec![item]))
        })
        .await
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        guard_async_result("incoming_calls", async move {
            // Call sites come from mir's posting lists; only the documents
            // containing them are parsed to resolve the enclosing caller.
            let docs = Arc::clone(&self.docs);
            let item_uri = params.item.uri.to_string();
            let item = params.item;
            let calls = match tokio::task::spawn_blocking(move || {
                // Pause the background scan and snapshot a settled revision so
                // only a genuine user edit cancels the lookup.
                let (_interactive, cancel_rev) = docs.settled_write_rev_guard();
                incoming_calls_indexed(&item, &docs, Some(cancel_rev))
            })
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("incoming_calls panicked for {item_uri}: {e}");
                    vec![]
                }
            };
            Ok(if calls.is_empty() { None } else { Some(calls) })
        })
        .await
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        guard_async_result("outgoing_calls", async move {
            // Per-callee declaration lookups go through `decls_by_name` — the old
            // path re-scanned the whole workspace once per distinct callee.
            let wi = self.workspace_index_async().await;
            let docs = Arc::clone(&self.docs);
            let item_uri = params.item.uri.to_string();
            let item = params.item;
            let calls = match tokio::task::spawn_blocking(move || {
                let get_doc = |u: &Url| docs.get_doc_salsa(u);
                outgoing_calls_indexed(&item, &wi, &get_doc)
            })
            .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("outgoing_calls panicked for {item_uri}: {e}");
                    vec![]
                }
            };
            Ok(if calls.is_empty() { None } else { Some(calls) })
        })
        .await
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        guard_async_result("document_highlight", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            // Stale-tolerant: a highlight fired on every cursor move must never
            // spin on the salsa `Cancelled` retry loop while the user types.
            let doc = match self.get_doc_stale(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let uri_str = uri.to_string();
            // The AST walk is CPU-bound; keep it off the async runtime worker.
            let highlights = match tokio::task::spawn_blocking(move || {
                document_highlights(&source, &doc, position)
            })
            .await
            {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!("document_highlight panicked for {uri_str}: {e}");
                    Vec::new()
                }
            };
            Ok(if highlights.is_empty() {
                None
            } else {
                Some(highlights)
            })
        })
        .await
    }

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        guard_async_result("linked_editing_range", async move {
            self.handle_linked_editing_range(params).await
        })
        .await
    }

    async fn goto_implementation(
        &self,
        params: tower_lsp::lsp_types::request::GotoImplementationParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoImplementationResponse>> {
        guard_async_result("goto_implementation", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let imports = self.file_imports(uri);
            let raw_word = crate::text::word_at_position(&source, position).unwrap_or_default();
            if raw_word.is_empty() {
                return Ok(None);
            }
            // `word_at_position` includes `\` as a word character, so the cursor on
            // a use-statement import (`use A\B\Foo`) returns the full qualified name.
            let (word, fqn): (String, String) = if raw_word.contains('\\') {
                let short = raw_word
                    .rsplit('\\')
                    .next()
                    .unwrap_or(&raw_word)
                    .to_string();
                (short, raw_word.trim_start_matches('\\').to_string())
            } else {
                // Resolve via this file's imports + namespace; covers usages,
                // aliases, and the type's own declaration (which resolves to
                // `<current-namespace>\<word>`).
                let resolved = match self.get_doc(uri) {
                    Some(doc) => crate::navigation::moniker::resolve_fqn(&doc, &raw_word, &imports),
                    None => raw_word.clone(),
                };
                (raw_word, resolved.trim_start_matches('\\').to_string())
            };

            // A method declaration name can collide case-insensitively with a
            // class name (`Guard` interface vs `guard()` method in one
            // namespace); the cursor context decides which sense wins.
            let on_method_decl = self.get_doc(uri).is_some_and(|doc| {
                cursor_is_on_method_decl(doc.source(), &doc.program().stmts, position)
            });

            // Subtypes of the type under the cursor, from mir's maintained
            // subtype edge index (aliased/FQN extends forms all resolve).
            let mut locs: Vec<Location> = if on_method_decl {
                Vec::new()
            } else {
                let docs = Arc::clone(&self.docs);
                let fqn_task = fqn.clone();
                tokio::task::spawn_blocking(move || docs.indexed_subtype_classes(&fqn_task, false))
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|site| subtype_site_to_location(&site.file, &site.range))
                    .collect()
            };

            // Cursor on a method name inside its declaring class/interface:
            // concrete overrides in every subtype.
            if locs.is_empty()
                && let Some(doc) = self.get_doc(uri)
                && let Some(enclosing) =
                    crate::types::type_map::enclosing_class_at(&source, &doc, position)
            {
                let enclosing_fqn =
                    crate::navigation::moniker::resolve_fqn(&doc, &enclosing, &imports)
                        .trim_start_matches('\\')
                        .to_string();
                let docs = Arc::clone(&self.docs);
                let method = word.clone();
                let impls = tokio::task::spawn_blocking(move || {
                    docs.indexed_method_implementations(&enclosing_fqn, &method)
                })
                .await
                .unwrap_or_default();
                locs = impls
                    .into_iter()
                    .filter_map(|(_, file, range)| subtype_site_to_location(&file, &range))
                    .collect();
            }
            if locs.is_empty() {
                Ok(None)
            } else {
                Ok(Some(GotoDefinitionResponse::Array(locs)))
            }
        })
        .await
    }

    async fn goto_declaration(
        &self,
        params: tower_lsp::lsp_types::request::GotoDeclarationParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoDeclarationResponse>> {
        guard_async_result("goto_declaration", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            // First pass: open-file ParsedDocs give accurate character positions.
            let open_docs = self.docs.docs_for(&self.open_urls());
            if let Some(loc) = goto_declaration(&source, &open_docs, position) {
                return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
            }
            // Second pass: background files via FileIndex (line-only positions).
            Ok(self
                .docs
                .with_all_indexes(|all_indexes| {
                    goto_declaration_from_index(&source, all_indexes, position)
                })
                .map(GotoDefinitionResponse::Scalar))
        })
        .await
    }

    async fn goto_type_definition(
        &self,
        params: tower_lsp::lsp_types::request::GotoTypeDefinitionParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoTypeDefinitionResponse>> {
        guard_async_result("goto_type_definition", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let analysis = self.cached_analysis_async(uri).await;
            let open_docs = self.docs.docs_for(&self.open_urls());

            // Exact FQN/namespace matches (open docs, then background index)
            // outrank *either* source's short-name fallback, so an unrelated
            // same-short-named class in another open file can never preempt
            // a correctly-namespaced match that only lives in the index.
            let mut results = goto_type_definition_exact(
                &source,
                &doc,
                analysis.as_deref(),
                &open_docs,
                position,
            );
            if results.is_empty() {
                results = self.docs.with_all_indexes(|all_indexes| {
                    goto_type_definition_from_index_exact(
                        &source,
                        &doc,
                        analysis.as_deref(),
                        all_indexes,
                        position,
                    )
                });
            }
            if results.is_empty() {
                results = goto_type_definition_short_name_fallback(
                    &source,
                    &doc,
                    analysis.as_deref(),
                    &open_docs,
                    position,
                );
            }
            if results.is_empty() {
                results = self.docs.with_all_indexes(|all_indexes| {
                    goto_type_definition_from_index_short_name_fallback(
                        &source,
                        &doc,
                        analysis.as_deref(),
                        all_indexes,
                        position,
                    )
                });
            }

            // Format response: scalar for single result, array for multiple, none for empty
            let response = match results.len() {
                0 => None,
                1 => Some(GotoDefinitionResponse::Scalar(
                    results.into_iter().next().unwrap(),
                )),
                _ => Some(GotoDefinitionResponse::Array(results)),
            };
            Ok(response)
        })
        .await
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        guard_async_result("prepare_type_hierarchy", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            // Phase J: use the salsa-memoized aggregate's `classes_by_name` map.
            let wi = self.workspace_index_async().await;
            Ok(
                prepare_type_hierarchy_from_workspace(&source, uri, &wi, position)
                    .map(|item| vec![item]),
            )
        })
        .await
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        guard_async_result("supertypes", async move {
            // Phase J: resolve parents via the aggregate's `classes_by_name` map.
            // Pre-load any direct vendor supertypes via PSR-4 so they appear in the
            // workspace index before the lookup runs.
            let wi = self.workspace_index_async().await;
            let loaded_new = self
                .ensure_direct_supertypes_loaded(&params.item.name, &wi)
                .await;
            let wi = if loaded_new {
                self.workspace_index_async().await
            } else {
                wi
            };
            let result = supertypes_of_from_workspace(&params.item, &wi);
            Ok(if result.is_empty() {
                None
            } else {
                Some(result)
            })
        })
        .await
    }

    async fn subtypes(
        &self,
        params: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        guard_async_result("subtypes", async move {
            let wi = self.workspace_index_async().await;
            // Resolve the item's FQCN for mir's subtype graph. `classes_by_name`
            // is keyed by short name only, so a name shared by many classes
            // across the workspace (e.g. Laravel's ~16 `Factory` classes) needs
            // `params.item.uri` to pick the right one instead of an arbitrary
            // first match.
            let item_fqn = wi
                .classes_by_name
                .get(&params.item.name)
                .and_then(|refs| {
                    refs.iter()
                        .filter_map(|r| wi.at(*r))
                        .find(|(u, _)| **u == params.item.uri)
                        .or_else(|| refs.first().and_then(|r| wi.at(*r)))
                })
                .map(|(_, cls)| cls.fqn.as_ref().to_string());
            let subtype_urls = item_fqn
                .as_deref()
                .map(|f| self.docs.class_subtype_urls(f))
                .unwrap_or_default();
            let result =
                subtypes_of_mir_backed(&params.item, item_fqn.as_deref(), &wi, &subtype_urls);
            Ok(if result.is_empty() {
                None
            } else {
                Some(result)
            })
        })
        .await
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        guard_async_result("code_lens", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            // Lens counts come from mir's inverted indexes; still run on the
            // blocking pool (index freshness passes may analyze on demand).
            // A write during computation (write_rev advances) cancels early —
            // stale lens counts are useless and the editor will request fresh
            // ones after the edit.
            let docs = Arc::clone(&self.docs);
            let docs_cancel = Arc::clone(&self.docs);
            let uri_owned = uri.clone();
            let uri_str = uri.to_string();
            let imports = self.file_imports(uri);
            let lenses = match tokio::task::spawn_blocking(move || {
                // Pause the background scan and snapshot a settled revision so
                // only a genuine user edit cancels the sweep.
                let (_interactive, cancel_rev) = docs.settled_write_rev_guard();
                code_lenses(
                    &uri_owned,
                    &doc,
                    &docs,
                    &imports,
                    Some(cancel_rev),
                    move || docs_cancel.write_rev() != cancel_rev,
                )
            })
            .await
            {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!("code_lens panicked for {uri_str}: {e}");
                    vec![]
                }
            };
            Ok(if lenses.is_empty() {
                None
            } else {
                Some(lenses)
            })
        })
        .await
    }

    async fn code_lens_resolve(&self, params: CodeLens) -> Result<CodeLens> {
        let fallback = params.clone();
        let resolved = guard_async("code_lens_resolve", async move {
            // Lenses are fully populated by code_lens; nothing to add.
            Some(params)
        })
        .await;
        Ok(resolved.unwrap_or(fallback))
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        guard_async_result("document_link", async move {
            let uri = &params.text_document.uri;
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let links = document_links(uri, &doc, doc.source());
            Ok(if links.is_empty() { None } else { Some(links) })
        })
        .await
    }

    async fn document_link_resolve(&self, params: DocumentLink) -> Result<DocumentLink> {
        let fallback = params.clone();
        let resolved = guard_async("document_link_resolve", async move {
            // Links already carry their target URI; nothing to add.
            Some(params)
        })
        .await;
        Ok(resolved.unwrap_or(fallback))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        guard_async_result("formatting", async move {
            let uri = &params.text_document.uri;
            let source = self.get_open_text(uri).unwrap_or_default();
            Ok(format_document(&source))
        })
        .await
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        guard_async_result("range_formatting", async move {
            let uri = &params.text_document.uri;
            let source = self.get_open_text(uri).unwrap_or_default();
            Ok(format_range(&source, params.range))
        })
        .await
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        guard_async_result("on_type_formatting", async move {
            let uri = &params.text_document_position.text_document.uri;
            let source = self.get_open_text(uri).unwrap_or_default();
            let edits = on_type_format(
                &source,
                params.text_document_position.position,
                &params.ch,
                &params.options,
            );
            Ok(if edits.is_empty() { None } else { Some(edits) })
        })
        .await
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        guard_async_result("execute_command", async move {
            match params.command.as_str() {
                "php-lsp.runTest" => {
                    // Arguments: [uri_string, "ClassName::methodName"]
                    let file_uri = params
                        .arguments
                        .first()
                        .and_then(|v| v.as_str())
                        .and_then(|s| Url::parse(s).ok());
                    let filter = params
                        .arguments
                        .get(1)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    let root = self.root_paths.load().first().cloned();
                    let client = self.client.clone();

                    tokio::spawn(async move {
                        run_phpunit(&client, &filter, root.as_deref(), file_uri.as_ref()).await;
                    });

                    Ok(None)
                }
                _ => Ok(None),
            }
        })
        .await
    }

    async fn will_rename_files(&self, params: RenameFilesParams) -> Result<Option<WorkspaceEdit>> {
        guard_async_result("will_rename_files", async move {
            self.handle_will_rename_files(params).await
        })
        .await
    }

    async fn did_rename_files(&self, params: RenameFilesParams) {
        guard_async("did_rename_files", async move {
            self.handle_did_rename_files(params).await
        })
        .await
    }

    async fn will_create_files(&self, params: CreateFilesParams) -> Result<Option<WorkspaceEdit>> {
        guard_async_result("will_create_files", async move {
            self.handle_will_create_files(params).await
        })
        .await
    }

    async fn did_create_files(&self, params: CreateFilesParams) {
        guard_async("did_create_files", async move {
            self.handle_did_create_files(params).await
        })
        .await
    }

    /// Before a file is deleted, return workspace edits that remove every
    /// `use` import referencing its PSR-4 class name.
    async fn will_delete_files(&self, params: DeleteFilesParams) -> Result<Option<WorkspaceEdit>> {
        guard_async_result("will_delete_files", async move {
            self.handle_will_delete_files(params).await
        })
        .await
    }

    async fn did_delete_files(&self, params: DeleteFilesParams) {
        guard_async("did_delete_files", async move {
            self.handle_did_delete_files(params).await
        })
        .await
    }

    async fn moniker(&self, params: MonikerParams) -> Result<Option<Vec<Moniker>>> {
        guard_async_result("moniker", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            let imports = self.file_imports(uri);
            Ok(moniker_at(&source, &doc, position, &imports).map(|m| vec![m]))
        })
        .await
    }

    async fn inline_value(&self, params: InlineValueParams) -> Result<Option<Vec<InlineValue>>> {
        guard_async_result("inline_value", async move {
            let uri = &params.text_document.uri;
            let source = self.get_open_text(uri).unwrap_or_default();
            let values = inline_values_in_range(&source, params.range);
            Ok(if values.is_empty() {
                None
            } else {
                Some(values)
            })
        })
        .await
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let report = guard_async("diagnostic", async move {
            Some(self.handle_diagnostic(params).await)
        })
        .await;
        match report {
            Some(r) => r,
            None => Ok(DocumentDiagnosticReportResult::Report(
                DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport::default()),
            )),
        }
    }

    async fn workspace_diagnostic(
        &self,
        params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let report = guard_async("workspace_diagnostic", async move {
            Some(self.handle_workspace_diagnostic(params).await)
        })
        .await;
        match report {
            Some(r) => r,
            None => Ok(WorkspaceDiagnosticReportResult::Report(
                WorkspaceDiagnosticReport::default(),
            )),
        }
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        guard_async_result("code_action", async move {
            self.handle_code_action(params).await
        })
        .await
    }

    async fn code_action_resolve(&self, item: CodeAction) -> Result<CodeAction> {
        guard_async_result("code_action_resolve", async move {
            self.handle_code_action_resolve(item).await
        })
        .await
    }
}

/// Convert a mir subtype/implementation hit — file path plus a name range in
/// mir coordinates (1-based line, 0-based char columns) — to an LSP Location.
fn subtype_site_to_location(file: &str, range: &mir_analyzer::Range) -> Option<Location> {
    let uri = Url::parse(file).ok()?;
    let line = range.start.line.saturating_sub(1);
    Some(Location {
        uri,
        range: tower_lsp::lsp_types::Range {
            start: tower_lsp::lsp_types::Position {
                line,
                character: range.start.column,
            },
            end: tower_lsp::lsp_types::Position {
                line,
                character: range.end.column,
            },
        },
    })
}
