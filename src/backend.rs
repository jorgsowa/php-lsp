use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::notification::Progress as ProgressNotification;
use tower_lsp::lsp_types::request::{
    CodeLensRefresh, InlayHintRefreshRequest, InlineValueRefreshRequest, SemanticTokensRefresh,
    WorkDoneProgressCreate, WorkspaceDiagnosticRefresh,
};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, async_trait};

use crate::ast::ParsedDoc;
use crate::autoload::Psr4Map;
use crate::call_hierarchy::{incoming_calls, outgoing_calls, prepare_call_hierarchy};
use crate::code_lens::code_lenses;
use crate::completion::filtered_completions_at;
use crate::declaration::goto_declaration;
use crate::definition::{find_declaration_range, goto_definition};
use crate::diagnostics::parse_document;
use crate::document_highlight::document_highlights;
use crate::document_link::document_links;
use crate::document_store::DocumentStore;
use crate::extract_action::{extract_method_actions, extract_variable_actions};
use crate::inline_action::inline_variable_actions;
use crate::organize_imports::organize_imports_action;
use crate::file_rename::{use_edits_for_delete, use_edits_for_rename};
use crate::folding::folding_ranges;
use crate::formatting::{format_document, format_range};
use crate::generate_action::{generate_constructor_actions, generate_getters_setters_actions};
use crate::hover::{docs_for_symbol, hover_info};
use crate::implement_action::implement_missing_actions;
use crate::implementation::goto_implementation;
use crate::inlay_hints::inlay_hints;
use crate::inline_value::inline_values_in_range;
use crate::moniker::moniker_at;
use crate::on_type_format::on_type_format;
use crate::phpdoc_action::phpdoc_actions;
use crate::type_action::add_return_type_actions;
use crate::phpstorm_meta::PhpStormMeta;
use crate::references::find_references;
use crate::rename::{prepare_rename, rename, rename_property, rename_variable};
use crate::selection_range::selection_ranges;
use crate::semantic_diagnostics::{
    deprecated_call_diagnostics, duplicate_declaration_diagnostics, semantic_diagnostics,
};
use crate::semantic_tokens::{
    compute_token_delta, legend, semantic_tokens, semantic_tokens_range, token_hash,
};
use crate::signature_help::signature_help;
use crate::symbols::{document_symbols, resolve_workspace_symbol, workspace_symbols};
use crate::type_definition::goto_type_definition;
use crate::type_hierarchy::{prepare_type_hierarchy, subtypes_of, supertypes_of};
use crate::util::word_at;

/// Per-category diagnostic toggle flags.
/// All flags default to `true` (enabled). Set to `false` to suppress that category.
#[derive(Debug, Clone)]
pub struct DiagnosticsConfig {
    /// Master switch: when `false`, no diagnostics are emitted.
    pub enabled: bool,
    /// Undefined variable references.
    pub undefined_variables: bool,
    /// Calls to undefined functions.
    pub undefined_functions: bool,
    /// References to undefined classes / interfaces / traits.
    pub undefined_classes: bool,
    /// Wrong number of arguments passed to a function.
    pub arity_errors: bool,
    /// Return-type mismatches.
    pub type_errors: bool,
    /// Calls to `@deprecated` members.
    pub deprecated_calls: bool,
    /// Duplicate class / function declarations.
    pub duplicate_declarations: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        DiagnosticsConfig {
            enabled: true,
            undefined_variables: true,
            undefined_functions: true,
            undefined_classes: true,
            arity_errors: true,
            type_errors: true,
            deprecated_calls: true,
            duplicate_declarations: true,
        }
    }
}

impl DiagnosticsConfig {
    fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = DiagnosticsConfig::default();
        let Some(obj) = v.as_object() else { return cfg };
        let flag = |key: &str| obj.get(key).and_then(|x| x.as_bool()).unwrap_or(true);
        cfg.enabled = flag("enabled");
        cfg.undefined_variables = flag("undefinedVariables");
        cfg.undefined_functions = flag("undefinedFunctions");
        cfg.undefined_classes = flag("undefinedClasses");
        cfg.arity_errors = flag("arityErrors");
        cfg.type_errors = flag("typeErrors");
        cfg.deprecated_calls = flag("deprecatedCalls");
        cfg.duplicate_declarations = flag("duplicateDeclarations");
        cfg
    }
}

/// Configuration received from the client via `initializationOptions`.
#[derive(Debug, Default, Clone)]
pub struct LspConfig {
    /// PHP version string, e.g. `"8.1"`.  Currently informational; future
    /// versions of the analyser will gate PHP-version-specific diagnostics.
    pub php_version: Option<String>,
    /// Glob patterns for paths to exclude from workspace indexing.
    pub exclude_paths: Vec<String>,
    /// Per-category diagnostic toggles.
    pub diagnostics: DiagnosticsConfig,
}

impl LspConfig {
    fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = LspConfig::default();
        if let Some(ver) = v.get("phpVersion").and_then(|x| x.as_str()) {
            cfg.php_version = Some(ver.to_string());
        }
        if let Some(arr) = v.get("excludePaths").and_then(|x| x.as_array()) {
            cfg.exclude_paths = arr
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect();
        }
        if let Some(diag_val) = v.get("diagnostics") {
            cfg.diagnostics = DiagnosticsConfig::from_value(diag_val);
        }
        cfg
    }
}

pub struct Backend {
    client: Client,
    docs: Arc<DocumentStore>,
    root_paths: Arc<RwLock<Vec<PathBuf>>>,
    psr4: Arc<RwLock<Psr4Map>>,
    meta: Arc<RwLock<PhpStormMeta>>,
    config: Arc<RwLock<LspConfig>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend {
            client,
            docs: Arc::new(DocumentStore::new()),
            root_paths: Arc::new(RwLock::new(Vec::new())),
            psr4: Arc::new(RwLock::new(Psr4Map::empty())),
            meta: Arc::new(RwLock::new(PhpStormMeta::default())),
            config: Arc::new(RwLock::new(LspConfig::default())),
        }
    }
}

#[async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Collect all workspace roots. Prefer workspace_folders (multi-root) over
        // the deprecated root_uri (single root).
        {
            let mut roots: Vec<PathBuf> = params
                .workspace_folders
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter_map(|f| f.uri.to_file_path().ok())
                .collect();
            if roots.is_empty()
                && let Some(path) = params.root_uri.as_ref().and_then(|u| u.to_file_path().ok())
            {
                roots.push(path);
            }
            *self.root_paths.write().unwrap() = roots;
        }

        // Parse initializationOptions if provided by the client.
        if let Some(opts) = &params.initialization_options {
            *self.config.write().unwrap() = LspConfig::from_value(opts);
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        will_save: Some(true),
                        will_save_wait_until: Some(true),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(false),
                        })),
                    },
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "$".to_string(),
                        ">".to_string(),
                        ":".to_string(),
                        "(".to_string(),
                        "[".to_string(),
                    ]),
                    resolve_provider: Some(true),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Right(WorkspaceSymbolOptions {
                    resolve_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
                    InlayHintOptions {
                        resolve_provider: Some(true),
                        work_done_progress_options: Default::default(),
                    },
                ))),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: legend(),
                            full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
                            range: Some(true),
                            ..Default::default()
                        },
                    ),
                ),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        resolve_provider: Some(true),
                        ..Default::default()
                    },
                )),
                declaration_provider: Some(DeclarationCapability::Simple(true)),
                type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: Some(true),
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
                    first_trigger_character: "}".to_string(),
                    more_trigger_character: Some(vec!["\n".to_string()]),
                }),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(true),
                    work_done_progress_options: Default::default(),
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![
                        "php-lsp.showReferences".to_string(),
                        "php-lsp.runTest".to_string(),
                    ],
                    work_done_progress_options: Default::default(),
                }),
                diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
                    DiagnosticOptions {
                        identifier: None,
                        inter_file_dependencies: true,
                        workspace_diagnostics: true,
                        work_done_progress_options: Default::default(),
                    },
                )),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                        will_rename: Some(php_file_op()),
                        did_rename: Some(php_file_op()),
                        did_create: Some(php_file_op()),
                        will_delete: Some(php_file_op()),
                        did_delete: Some(php_file_op()),
                        ..Default::default()
                    }),
                }),
                linked_editing_range_provider: Some(LinkedEditingRangeServerCapabilities::Simple(
                    true,
                )),
                moniker_provider: Some(OneOf::Left(true)),
                inline_value_provider: Some(OneOf::Right(InlineValueServerCapabilities::Options(
                    InlineValueOptions {
                        work_done_progress_options: Default::default(),
                    },
                ))),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        // Register dynamic capabilities: file watcher + type hierarchy
        let php_selector = serde_json::json!([{"language": "php"}]);
        let registrations = vec![
            Registration {
                id: "php-lsp-file-watcher".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(
                    serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                        watchers: vec![FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.php".to_string()),
                            kind: None,
                        }],
                    })
                    .unwrap(),
                ),
            },
            // Type hierarchy has no static ServerCapabilities field in lsp-types 0.94,
            // so register it dynamically here.
            Registration {
                id: "php-lsp-type-hierarchy".to_string(),
                method: "textDocument/prepareTypeHierarchy".to_string(),
                register_options: Some(serde_json::json!({"documentSelector": php_selector})),
            },
            // Watch for configuration changes so we can pull the latest settings.
            Registration {
                id: "php-lsp-config-change".to_string(),
                method: "workspace/didChangeConfiguration".to_string(),
                register_options: Some(serde_json::json!({"section": "php-lsp"})),
            },
        ];
        self.client.register_capability(registrations).await.ok();

        // Load PSR-4 autoload map and kick off background workspace scan.
        // Extract roots first so RwLockReadGuard is dropped before any .await.
        let roots = self.root_paths.read().unwrap().clone();
        if !roots.is_empty() {
            // Build PSR-4 map from all roots (entries from all roots are merged).
            {
                let mut merged = Psr4Map::empty();
                for root in &roots {
                    merged.extend(Psr4Map::load(root));
                }
                *self.psr4.write().unwrap() = merged;
            }
            // Load PHPStorm metadata from the first root, if present.
            *self.meta.write().unwrap() = PhpStormMeta::load(&roots[0]);

            // Create a client-side progress indicator for the workspace scan.
            let token = NumberOrString::String("php-lsp/indexing".to_string());
            self.client
                .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                    token: token.clone(),
                })
                .await
                .ok();

            let docs = Arc::clone(&self.docs);
            let client = self.client.clone();
            let exclude_paths = self.config.read().unwrap().exclude_paths.clone();
            tokio::spawn(async move {
                client
                    .send_notification::<ProgressNotification>(ProgressParams {
                        token: token.clone(),
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                            WorkDoneProgressBegin {
                                title: "php-lsp: indexing workspace".to_string(),
                                cancellable: Some(false),
                                message: None,
                                percentage: None,
                            },
                        )),
                    })
                    .await;

                let mut total = 0usize;
                for root in roots {
                    total += scan_workspace(root, Arc::clone(&docs), &exclude_paths).await;
                }

                client
                    .send_notification::<ProgressNotification>(ProgressParams {
                        token,
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(
                            WorkDoneProgressEnd {
                                message: Some(format!("Indexed {total} files")),
                            },
                        )),
                    })
                    .await;

                client
                    .log_message(
                        MessageType::INFO,
                        format!("php-lsp: indexed {total} workspace files"),
                    )
                    .await;

                // Ask clients to re-request tokens/lenses/hints/diagnostics now
                // that the index is populated. Without this, editors that opened
                // files before indexing finished would show stale information.
                send_refresh_requests(&client).await;
            });
        }

        self.client
            .log_message(MessageType::INFO, "php-lsp ready")
            .await;
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        // Pull the current configuration from the client rather than parsing the
        // (often-null) params.settings, which not all clients populate.
        let items = vec![ConfigurationItem {
            scope_uri: None,
            section: Some("php-lsp".to_string()),
        }];
        if let Ok(values) = self.client.configuration(items).await
            && let Some(value) = values.into_iter().next()
        {
            *self.config.write().unwrap() = LspConfig::from_value(&value);
        }
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        // Remove folders from our tracked roots.
        {
            let mut roots = self.root_paths.write().unwrap();
            for removed in &params.event.removed {
                if let Ok(path) = removed.uri.to_file_path() {
                    roots.retain(|r| r != &path);
                }
            }
        }

        // Add new folders and kick off background scans for each.
        let exclude_paths = self.config.read().unwrap().exclude_paths.clone();
        for added in &params.event.added {
            if let Ok(path) = added.uri.to_file_path() {
                {
                    let mut roots = self.root_paths.write().unwrap();
                    if !roots.contains(&path) {
                        roots.push(path.clone());
                    }
                }
                let docs = Arc::clone(&self.docs);
                let ex = exclude_paths.clone();
                let path_clone = path.clone();
                let client = self.client.clone();
                tokio::spawn(async move {
                    scan_workspace(path_clone, docs, &ex).await;
                    send_refresh_requests(&client).await;
                });
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;

        // Store text immediately so other features work while parsing
        let version = self.docs.set_text(uri.clone(), text.clone());

        // Parse in a blocking thread to avoid stalling the tokio runtime;
        // await here so the AST is ready before the handler returns.
        let (doc, diagnostics) = tokio::task::spawn_blocking(move || parse_document(&text))
            .await
            .unwrap_or_else(|_| (ParsedDoc::default(), vec![]));

        self.docs
            .apply_parse(&uri, doc, diagnostics.clone(), version);
        let stored_source = self.docs.get(&uri).unwrap_or_default();
        let doc2 = self.docs.get_doc(&uri);
        let mut all_diags = diagnostics;
        if let Some(ref d) = doc2 {
            let diag_cfg = self.config.read().unwrap().diagnostics.clone();
            let dup_diags = duplicate_declaration_diagnostics(&stored_source, d, &diag_cfg);
            all_diags.extend(dup_diags);
        }
        self.client.publish_diagnostics(uri, all_diags, None).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = match params.content_changes.into_iter().last() {
            Some(c) => c.text,
            None => return,
        };

        // Store text immediately and capture the version token.
        // Features (completion, hover, …) see the new text instantly while
        // the parse runs in the background.
        let version = self.docs.set_text(uri.clone(), text.clone());

        let docs = Arc::clone(&self.docs);
        let client = self.client.clone();
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();
        tokio::spawn(async move {
            // 100 ms debounce: if another edit arrives before we parse, the
            // version check in apply_parse will discard this stale result.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let (doc, diagnostics) = tokio::task::spawn_blocking(move || parse_document(&text))
                .await
                .unwrap_or_else(|_| (ParsedDoc::default(), vec![]));

            // Only apply if no newer edit arrived while we were parsing
            if docs.apply_parse(&uri, doc, diagnostics.clone(), version) {
                let source = docs.get(&uri).unwrap_or_default();
                let mut all_diags = diagnostics;
                if let Some(d) = docs.get_doc(&uri) {
                    all_diags.extend(duplicate_declaration_diagnostics(&source, &d, &diag_cfg));
                    let other_raw = docs.other_docs(&uri);
                    let other_docs: Vec<Arc<ParsedDoc>> =
                        other_raw.into_iter().map(|(_, d)| d).collect();
                    all_diags.extend(deprecated_call_diagnostics(
                        &source,
                        &d,
                        &other_docs,
                        &diag_cfg,
                    ));
                }
                client.publish_diagnostics(uri, all_diags, None).await;
            }
        });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.close(&uri);
        // Clear editor diagnostics; the file stays indexed for cross-file features
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn will_save(&self, _params: WillSaveTextDocumentParams) {}

    async fn will_save_wait_until(
        &self,
        params: WillSaveTextDocumentParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let source = self.docs.get(&params.text_document.uri).unwrap_or_default();
        Ok(format_document(&source))
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        // Re-publish diagnostics on save so editors that defer diagnostics
        // until save (rather than on every keystroke) see up-to-date results.
        let source = self.docs.get(&uri).unwrap_or_default();
        let doc = self.docs.get_doc(&uri);
        if let Some(ref d) = doc {
            let diag_cfg = self.config.read().unwrap().diagnostics.clone();
            let parse_diags = self.docs.get_diagnostics(&uri).unwrap_or_default();
            let dup_diags = duplicate_declaration_diagnostics(&source, d, &diag_cfg);
            let other_raw = self.docs.other_docs(&uri);
            let other_docs: Vec<Arc<ParsedDoc>> = other_raw.into_iter().map(|(_, d)| d).collect();
            let dep_diags = deprecated_call_diagnostics(&source, d, &other_docs, &diag_cfg);
            let mut all = parse_diags;
            all.extend(dup_diags);
            all.extend(dep_diags);
            self.client.publish_diagnostics(uri, all, None).await;
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            match change.typ {
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    if let Ok(path) = change.uri.to_file_path()
                        && let Ok(text) = tokio::fs::read_to_string(&path).await
                    {
                        self.docs.index(change.uri, &text);
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
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(Some(CompletionResponse::Array(vec![]))),
        };
        let other_docs: Vec<Arc<ParsedDoc>> = self
            .docs
            .other_docs(uri)
            .into_iter()
            .map(|(_, d)| d)
            .collect();
        let trigger = params
            .context
            .as_ref()
            .and_then(|c| c.trigger_character.as_deref());
        let meta_guard = self.meta.read().unwrap();
        let meta_opt = if meta_guard.is_empty() {
            None
        } else {
            Some(&*meta_guard)
        };
        Ok(Some(CompletionResponse::Array(filtered_completions_at(
            &doc,
            &other_docs,
            trigger,
            Some(&source),
            Some(position),
            meta_opt,
            Some(uri),
        ))))
    }

    async fn completion_resolve(&self, mut item: CompletionItem) -> Result<CompletionItem> {
        if item.documentation.is_some() {
            return Ok(item);
        }
        // Strip trailing ':' from named-argument labels (e.g. "param:") before lookup.
        let name = item.label.trim_end_matches(':');
        let all_docs = self.docs.all_docs();
        if let Some(md) = docs_for_symbol(name, &all_docs) {
            item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }));
        }
        Ok(item)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let other_docs = self.docs.other_docs(uri);

        // Primary lookup: search all indexed documents
        if let Some(loc) = goto_definition(uri, &source, &doc, &other_docs, position) {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }

        // PSR-4 fallback: only useful for fully-qualified names (contain `\`)
        if let Some(word) = word_at(&source, position)
            && word.contains('\\')
            && let Some(loc) = self.psr4_goto(&word).await
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }

        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let word = match word_at(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        let all_docs = self.docs.all_docs();
        let include_declaration = params.context.include_declaration;
        let locations = find_references(&word, &all_docs, include_declaration);
        Ok(if locations.is_empty() {
            None
        } else {
            Some(locations)
        })
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();
        Ok(prepare_rename(&source, params.position).map(PrepareRenameResponse::Range))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let word = match word_at(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        if word.starts_with('$') {
            let doc = match self.docs.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            Ok(Some(rename_variable(&word, &params.new_name, uri, &source, &doc, position)))
        } else if is_after_arrow(&source, position) {
            let all_docs = self.docs.all_docs();
            Ok(Some(rename_property(&word, &params.new_name, &all_docs)))
        } else {
            let all_docs = self.docs.all_docs();
            Ok(Some(rename(&word, &params.new_name, &all_docs)))
        }
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(signature_help(&source, &doc, position))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let other_docs = self.docs.other_docs(uri);
        Ok(hover_info(&source, &doc, position, &other_docs))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(DocumentSymbolResponse::Nested(document_symbols(
            doc.source(),
            &doc,
        ))))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let ranges = folding_ranges(doc.source(), &doc);
        Ok(if ranges.is_empty() {
            None
        } else {
            Some(ranges)
        })
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(Some(inlay_hints(doc.source(), &doc, params.range)))
    }

    async fn inlay_hint_resolve(&self, mut item: InlayHint) -> Result<InlayHint> {
        if item.tooltip.is_some() {
            return Ok(item);
        }
        let func_name = item
            .data
            .as_ref()
            .and_then(|d| d.get("php_lsp_fn"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(name) = func_name {
            let all_docs = self.docs.all_docs();
            if let Some(md) = docs_for_symbol(&name, &all_docs) {
                item.tooltip = Some(InlayHintTooltip::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: md,
                }));
            }
        }
        Ok(item)
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let docs = self.docs.all_docs();
        let results = workspace_symbols(&params.query, &docs);
        Ok(if results.is_empty() {
            None
        } else {
            Some(results)
        })
    }

    async fn symbol_resolve(&self, params: WorkspaceSymbol) -> Result<WorkspaceSymbol> {
        let docs = self.docs.all_docs();
        Ok(resolve_workspace_symbol(params, &docs))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => {
                return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                    result_id: None,
                    data: vec![],
                })));
            }
        };
        let tokens = semantic_tokens(doc.source(), &doc);
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }

    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokensRangeResult>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
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
    }

    async fn semantic_tokens_full_delta(
        &self,
        params: SemanticTokensDeltaParams,
    ) -> Result<Option<SemanticTokensFullDeltaResult>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };

        let new_tokens = semantic_tokens(doc.source(), &doc);
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
                data: new_tokens.clone(),
            }),
        };

        self.docs.store_token_cache(uri, new_result_id, new_tokens);
        Ok(Some(result))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let ranges = selection_ranges(doc.source(), &doc, &params.positions);
        Ok(if ranges.is_empty() {
            None
        } else {
            Some(ranges)
        })
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let word = match word_at(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        let all_docs = self.docs.all_docs();
        Ok(prepare_call_hierarchy(&word, &all_docs).map(|item| vec![item]))
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let all_docs = self.docs.all_docs();
        let calls = incoming_calls(&params.item, &all_docs);
        Ok(if calls.is_empty() { None } else { Some(calls) })
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let all_docs = self.docs.all_docs();
        let calls = outgoing_calls(&params.item, &all_docs);
        Ok(if calls.is_empty() { None } else { Some(calls) })
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let highlights = document_highlights(&source, &doc, position);
        Ok(if highlights.is_empty() {
            None
        } else {
            Some(highlights)
        })
    }

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        // Reuse document_highlights: every occurrence of the symbol is a linked range.
        let highlights = document_highlights(&source, &doc, position);
        if highlights.is_empty() {
            return Ok(None);
        }
        let ranges: Vec<Range> = highlights.into_iter().map(|h| h.range).collect();
        Ok(Some(LinkedEditingRanges {
            ranges,
            // PHP identifiers: letters, digits, underscore; variables also allow leading $
            word_pattern: Some(r"[$a-zA-Z_\x80-\xff][a-zA-Z0-9_\x80-\xff]*".to_string()),
        }))
    }

    async fn goto_implementation(
        &self,
        params: tower_lsp::lsp_types::request::GotoImplementationParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoImplementationResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let all_docs = self.docs.all_docs();
        let locs = goto_implementation(&source, &all_docs, position);
        if locs.is_empty() {
            Ok(None)
        } else {
            Ok(Some(GotoDefinitionResponse::Array(locs)))
        }
    }

    async fn goto_declaration(
        &self,
        params: tower_lsp::lsp_types::request::GotoDeclarationParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoDeclarationResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let all_docs = self.docs.all_docs();
        Ok(goto_declaration(&source, &all_docs, position).map(GotoDefinitionResponse::Scalar))
    }

    async fn goto_type_definition(
        &self,
        params: tower_lsp::lsp_types::request::GotoTypeDefinitionParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoTypeDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let all_docs = self.docs.all_docs();
        Ok(goto_type_definition(&source, &doc, &all_docs, position)
            .map(GotoDefinitionResponse::Scalar))
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let all_docs = self.docs.all_docs();
        Ok(prepare_type_hierarchy(&source, &all_docs, position).map(|item| vec![item]))
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let all_docs = self.docs.all_docs();
        let result = supertypes_of(&params.item, &all_docs);
        Ok(if result.is_empty() {
            None
        } else {
            Some(result)
        })
    }

    async fn subtypes(
        &self,
        params: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let all_docs = self.docs.all_docs();
        let result = subtypes_of(&params.item, &all_docs);
        Ok(if result.is_empty() {
            None
        } else {
            Some(result)
        })
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let all_docs = self.docs.all_docs();
        let lenses = code_lenses(uri, &doc, &all_docs);
        Ok(if lenses.is_empty() {
            None
        } else {
            Some(lenses)
        })
    }

    async fn code_lens_resolve(&self, params: CodeLens) -> Result<CodeLens> {
        // Lenses are fully populated by code_lens; nothing to add.
        Ok(params)
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let links = document_links(uri, &doc, doc.source());
        Ok(if links.is_empty() { None } else { Some(links) })
    }

    async fn document_link_resolve(&self, params: DocumentLink) -> Result<DocumentLink> {
        // Links already carry their target URI; nothing to add.
        Ok(params)
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();
        Ok(format_document(&source))
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();
        Ok(format_range(&source, params.range))
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document_position.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();
        let edits = on_type_format(
            &source,
            params.text_document_position.position,
            &params.ch,
            &params.options,
        );
        Ok(if edits.is_empty() { None } else { Some(edits) })
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        match params.command.as_str() {
            "php-lsp.showReferences" => {
                // The client handles showing the references panel;
                // the server just acknowledges the command.
                Ok(None)
            }
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

                let root = self.root_paths.read().unwrap().first().cloned();
                let client = self.client.clone();

                tokio::spawn(async move {
                    run_phpunit(&client, &filter, root.as_deref(), file_uri.as_ref()).await;
                });

                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn will_rename_files(&self, params: RenameFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.read().unwrap();
        let all_docs = self.docs.all_docs();
        let mut merged_changes: std::collections::HashMap<
            tower_lsp::lsp_types::Url,
            Vec<tower_lsp::lsp_types::TextEdit>,
        > = std::collections::HashMap::new();

        for file_rename in &params.files {
            let old_path = tower_lsp::lsp_types::Url::parse(&file_rename.old_uri)
                .ok()
                .and_then(|u| u.to_file_path().ok());
            let new_path = tower_lsp::lsp_types::Url::parse(&file_rename.new_uri)
                .ok()
                .and_then(|u| u.to_file_path().ok());

            let (Some(old_path), Some(new_path)) = (old_path, new_path) else {
                continue;
            };

            let old_fqn = psr4.file_to_fqn(&old_path);
            let new_fqn = psr4.file_to_fqn(&new_path);

            let (Some(old_fqn), Some(new_fqn)) = (old_fqn, new_fqn) else {
                continue;
            };

            let edit = use_edits_for_rename(&old_fqn, &new_fqn, &all_docs);
            if let Some(changes) = edit.changes {
                for (uri, edits) in changes {
                    merged_changes.entry(uri).or_default().extend(edits);
                }
            }
        }

        Ok(if merged_changes.is_empty() {
            None
        } else {
            Some(WorkspaceEdit {
                changes: Some(merged_changes),
                ..Default::default()
            })
        })
    }

    async fn did_rename_files(&self, params: RenameFilesParams) {
        for file_rename in &params.files {
            // Drop the old URI from the index
            if let Ok(old_uri) = tower_lsp::lsp_types::Url::parse(&file_rename.old_uri) {
                self.docs.remove(&old_uri);
            }
            // Index the file at its new location
            if let Ok(new_uri) = tower_lsp::lsp_types::Url::parse(&file_rename.new_uri)
                && let Ok(path) = new_uri.to_file_path()
                && let Ok(text) = tokio::fs::read_to_string(&path).await
            {
                self.docs.index(new_uri, &text);
            }
        }
    }

    // ── File-create notifications ────────────────────────────────────────────

    async fn will_create_files(&self, params: CreateFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.read().unwrap();
        let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file in &params.files {
            let Ok(uri) = Url::parse(&file.uri) else {
                continue;
            };
            let Ok(path) = uri.to_file_path() else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some("php") {
                continue;
            }

            let stub = if let Some(fqn) = psr4.file_to_fqn(&path) {
                let (ns, class_name) = match fqn.rfind('\\') {
                    Some(pos) => (&fqn[..pos], &fqn[pos + 1..]),
                    None => ("", fqn.as_str()),
                };
                if ns.is_empty() {
                    format!("<?php\n\ndeclare(strict_types=1);\n\nclass {class_name}\n{{\n}}\n")
                } else {
                    format!(
                        "<?php\n\ndeclare(strict_types=1);\n\nnamespace {ns};\n\nclass {class_name}\n{{\n}}\n"
                    )
                }
            } else {
                "<?php\n\n".to_string()
            };

            changes.insert(
                uri,
                vec![TextEdit {
                    range: Range {
                        start: Position { line: 0, character: 0 },
                        end: Position { line: 0, character: 0 },
                    },
                    new_text: stub,
                }],
            );
        }

        Ok(if changes.is_empty() {
            None
        } else {
            Some(WorkspaceEdit { changes: Some(changes), ..Default::default() })
        })
    }

    async fn did_create_files(&self, params: CreateFilesParams) {
        for file in &params.files {
            if let Ok(uri) = Url::parse(&file.uri)
                && let Ok(path) = uri.to_file_path()
                && let Ok(text) = tokio::fs::read_to_string(&path).await
            {
                self.docs.index(uri, &text);
            }
        }
        send_refresh_requests(&self.client).await;
    }

    // ── File-delete notifications ────────────────────────────────────────────

    /// Before a file is deleted, return workspace edits that remove every
    /// `use` import referencing its PSR-4 class name.
    async fn will_delete_files(&self, params: DeleteFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.read().unwrap();
        let all_docs = self.docs.all_docs();
        let mut merged_changes: std::collections::HashMap<Url, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file in &params.files {
            let path = Url::parse(&file.uri)
                .ok()
                .and_then(|u| u.to_file_path().ok());
            let Some(path) = path else { continue };
            let Some(fqn) = psr4.file_to_fqn(&path) else {
                continue;
            };

            let edit = use_edits_for_delete(&fqn, &all_docs);
            if let Some(changes) = edit.changes {
                for (uri, edits) in changes {
                    merged_changes.entry(uri).or_default().extend(edits);
                }
            }
        }

        Ok(if merged_changes.is_empty() {
            None
        } else {
            Some(WorkspaceEdit {
                changes: Some(merged_changes),
                ..Default::default()
            })
        })
    }

    async fn did_delete_files(&self, params: DeleteFilesParams) {
        for file in &params.files {
            if let Ok(uri) = Url::parse(&file.uri) {
                self.docs.remove(&uri);
                // Clear diagnostics for the now-deleted file.
                self.client.publish_diagnostics(uri, vec![], None).await;
            }
        }
        send_refresh_requests(&self.client).await;
    }

    // ── Moniker ──────────────────────────────────────────────────────────────

    async fn moniker(&self, params: MonikerParams) -> Result<Option<Vec<Moniker>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        Ok(moniker_at(&source, &doc, position).map(|m| vec![m]))
    }

    // ── Inline values ────────────────────────────────────────────────────────

    async fn inline_value(&self, params: InlineValueParams) -> Result<Option<Vec<InlineValue>>> {
        let uri = &params.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();
        let values = inline_values_in_range(&source, params.range);
        Ok(if values.is_empty() {
            None
        } else {
            Some(values)
        })
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();

        let parse_diags = self.docs.get_diagnostics(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => {
                return Ok(DocumentDiagnosticReportResult::Report(
                    DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                        related_documents: None,
                        full_document_diagnostic_report: FullDocumentDiagnosticReport {
                            result_id: None,
                            items: parse_diags,
                        },
                    }),
                ));
            }
        };
        let other_docs = self.docs.other_docs(uri);
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();
        let sem_diags = semantic_diagnostics(uri, &doc, &other_docs, &diag_cfg);
        let dup_diags = duplicate_declaration_diagnostics(&source, &doc, &diag_cfg);

        let mut items = parse_diags;
        items.extend(sem_diags);
        items.extend(dup_diags);

        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items,
                },
            }),
        ))
    }

    async fn workspace_diagnostic(
        &self,
        _params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let all_docs = self.docs.all_docs();
        let all_parse_diags = self.docs.all_diagnostics();
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();

        let items: Vec<WorkspaceDocumentDiagnosticReport> = all_parse_diags
            .into_iter()
            .filter_map(|(uri, parse_diags, version)| {
                let doc = self.docs.get_doc(&uri)?;

                // Build other_docs by filtering the current URI out of all_docs.
                let other_docs: Vec<(Url, Arc<ParsedDoc>)> = all_docs
                    .iter()
                    .filter(|(u, _)| u != &uri)
                    .cloned()
                    .collect();

                let source = doc.source().to_string();
                let sem_diags = semantic_diagnostics(&uri, &doc, &other_docs, &diag_cfg);
                let dup_diags = duplicate_declaration_diagnostics(&source, &doc, &diag_cfg);

                let mut all_diags = parse_diags;
                all_diags.extend(sem_diags);
                all_diags.extend(dup_diags);

                Some(WorkspaceDocumentDiagnosticReport::Full(
                    WorkspaceFullDocumentDiagnosticReport {
                        uri,
                        version,
                        full_document_diagnostic_report: FullDocumentDiagnosticReport {
                            result_id: None,
                            items: all_diags,
                        },
                    },
                ))
            })
            .collect();

        Ok(WorkspaceDiagnosticReportResult::Report(
            WorkspaceDiagnosticReport { items },
        ))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let other_docs = self.docs.other_docs(uri);

        // Semantic diagnostics — collect undefined symbols and offer "Add use import"
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();
        let sem_diags = semantic_diagnostics(uri, &doc, &other_docs, &diag_cfg);

        // Publish semantic diagnostics merged with existing parse diagnostics
        if !sem_diags.is_empty() {
            let mut all_diags = self.docs.get_diagnostics(uri).unwrap_or_default();
            all_diags.extend(sem_diags.clone());
            self.client
                .publish_diagnostics(uri.clone(), all_diags, None)
                .await;
        }

        // Build "Add use import" code actions for undefined class names in range
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        for diag in &sem_diags {
            if !diag.message.starts_with("Undefined:") {
                continue;
            }
            // Only act on diagnostics within the requested range
            if diag.range.start.line < params.range.start.line
                || diag.range.start.line > params.range.end.line
            {
                continue;
            }
            let class_name = diag
                .message
                .strip_prefix("Undefined: ")
                .unwrap_or("")
                .trim();
            if class_name.is_empty() {
                continue;
            }

            // Find a class with this short name in other indexed documents
            for (other_uri, other_doc) in &other_docs {
                if let Some(fqn) = find_fqn_for_class(other_doc, class_name, other_uri) {
                    let edit = build_use_import_edit(&source, uri, &fqn);
                    let action = CodeAction {
                        title: format!("Add use {fqn}"),
                        kind: Some(CodeActionKind::QUICKFIX),
                        edit: Some(edit),
                        diagnostics: Some(vec![diag.clone()]),
                        ..Default::default()
                    };
                    actions.push(CodeActionOrCommand::CodeAction(action));
                    break; // one action per undefined symbol
                }
            }
        }

        // PHPDoc, implement, constructor, getters/setters: defer edit computation to
        // code_action_resolve so the menu appears instantly.
        actions.extend(defer_actions(
            phpdoc_actions(uri, &doc, &source, params.range),
            "phpdoc",
            uri,
            params.range,
        ));
        actions.extend(defer_actions(
            implement_missing_actions(&source, &doc, &other_docs, params.range, uri),
            "implement",
            uri,
            params.range,
        ));
        actions.extend(defer_actions(
            generate_constructor_actions(&source, &doc, params.range, uri),
            "constructor",
            uri,
            params.range,
        ));
        actions.extend(defer_actions(
            generate_getters_setters_actions(&source, &doc, params.range, uri),
            "getters_setters",
            uri,
            params.range,
        ));

        actions.extend(defer_actions(
            add_return_type_actions(&source, &doc, params.range, uri),
            "return_type",
            uri,
            params.range,
        ));

        // Extract variable: cheap, keep eager.
        actions.extend(extract_variable_actions(&source, params.range, uri));
        actions.extend(extract_method_actions(&source, &doc, params.range, uri));
        // Inline variable: inverse of extract variable.
        actions.extend(inline_variable_actions(&source, params.range, uri));
        // Organize imports: sort and remove unused use statements.
        if let Some(action) = organize_imports_action(&source, uri) {
            actions.push(action);
        }

        Ok(if actions.is_empty() {
            None
        } else {
            Some(actions)
        })
    }

    async fn code_action_resolve(&self, item: CodeAction) -> Result<CodeAction> {
        let data = match &item.data {
            Some(d) => d.clone(),
            None => return Ok(item),
        };
        let kind_tag = match data.get("php_lsp_resolve").and_then(|v| v.as_str()) {
            Some(k) => k.to_string(),
            None => return Ok(item),
        };
        let uri: Url = match data
            .get("uri")
            .and_then(|v| v.as_str())
            .and_then(|s| Url::parse(s).ok())
        {
            Some(u) => u,
            None => return Ok(item),
        };
        let range: Range = match data
            .get("range")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
        {
            Some(r) => r,
            None => return Ok(item),
        };

        let source = self.docs.get(&uri).unwrap_or_default();
        let doc = match self.docs.get_doc(&uri) {
            Some(d) => d,
            None => return Ok(item),
        };

        let candidates: Vec<CodeActionOrCommand> = match kind_tag.as_str() {
            "phpdoc" => phpdoc_actions(&uri, &doc, &source, range),
            "implement" => {
                let other_docs = self.docs.other_docs(&uri);
                implement_missing_actions(&source, &doc, &other_docs, range, &uri)
            }
            "constructor" => generate_constructor_actions(&source, &doc, range, &uri),
            "getters_setters" => generate_getters_setters_actions(&source, &doc, range, &uri),
            "return_type" => add_return_type_actions(&source, &doc, range, &uri),
            _ => return Ok(item),
        };

        // Find the action whose title matches and return it fully resolved.
        for candidate in candidates {
            if let CodeActionOrCommand::CodeAction(ca) = candidate
                && ca.title == item.title
            {
                return Ok(ca);
            }
        }

        Ok(item)
    }
}

/// Shorthand for a `FileOperationRegistrationOptions` that matches `*.php` files.
fn php_file_op() -> FileOperationRegistrationOptions {
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
fn defer_actions(
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

/// Find the fully-qualified name for a class with the given short `name` by
/// walking the ParsedDoc AST. Returns `Namespace\ClassName` when inside a namespace.
fn find_fqn_for_class(doc: &ParsedDoc, name: &str, _uri: &Url) -> Option<String> {
    use php_ast::{NamespaceBody, StmtKind};
    for stmt in doc.program().stmts.iter() {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(name) => {
                return Some(name.to_string());
            }
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().to_string());
                if let NamespaceBody::Braced(inner) = &ns.body {
                    for inner_stmt in inner.iter() {
                        if let StmtKind::Class(c) = &inner_stmt.kind
                            && c.name == Some(name)
                        {
                            return Some(match ns_name {
                                Some(ref ns) => format!("{ns}\\{name}"),
                                None => name.to_string(),
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Build a `WorkspaceEdit` that inserts `use FQN;` near the top of the file.
fn build_use_import_edit(source: &str, uri: &Url, fqn: &str) -> WorkspaceEdit {
    use std::collections::HashMap;
    // Insert after the `<?php` line and any existing `use` / `namespace` lines
    let insert_line = find_use_insert_line(source);
    let insert_text = format!("use {fqn};\n");
    let pos = tower_lsp::lsp_types::Position {
        line: insert_line,
        character: 0,
    };
    let edit = tower_lsp::lsp_types::TextEdit {
        range: tower_lsp::lsp_types::Range {
            start: pos,
            end: pos,
        },
        new_text: insert_text,
    };
    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![edit]);
    WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }
}

fn find_use_insert_line(source: &str) -> u32 {
    let mut last_use_or_ns: u32 = 0;
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("<?php")
            || trimmed.starts_with("namespace ")
            || trimmed.starts_with("use ")
        {
            last_use_or_ns = i as u32 + 1;
        }
    }
    last_use_or_ns
}

/// Returns `true` when the identifier at `position` is immediately preceded by `->`,
/// indicating it is a property or method name in an instance access expression.
fn is_after_arrow(source: &str, position: Position) -> bool {
    let line = match source.lines().nth(position.line as usize) {
        Some(l) => l,
        None => return false,
    };
    let chars: Vec<char> = line.chars().collect();
    let col = position.character as usize;
    // Find the char index of the cursor (UTF-16 → char index).
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16_col >= col {
            break;
        }
        utf16_col += ch.len_utf16();
        char_idx += 1;
    }
    // Walk left past word chars to the start of the identifier.
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx > 0 && is_word(chars[char_idx - 1]) {
        char_idx -= 1;
    }
    char_idx >= 2 && chars[char_idx - 1] == '>' && chars[char_idx - 2] == '-'
}

impl Backend {
    /// Try to resolve a fully-qualified name via the PSR-4 map.
    /// Indexes the file on-demand if it is not already in the document store.
    async fn psr4_goto(&self, fqn: &str) -> Option<Location> {
        let path = {
            let psr4 = self.psr4.read().unwrap();
            psr4.resolve(fqn)?
        };

        let file_uri = Url::from_file_path(&path).ok()?;

        // Index on-demand if the file was not picked up by the workspace scan
        if self.docs.get_doc(&file_uri).is_none() {
            let text = tokio::fs::read_to_string(&path).await.ok()?;
            self.docs.index(file_uri.clone(), &text);
        }

        let doc = self.docs.get_doc(&file_uri)?;

        // Classes are declared by their short (unqualified) name, e.g. `class Foo`
        // not `class App\Services\Foo`.
        let short_name = fqn.split('\\').next_back()?;
        let range = find_declaration_range(doc.source(), &doc, short_name)?;

        Some(Location {
            uri: file_uri,
            range,
        })
    }
}

/// Run `vendor/bin/phpunit --filter <filter>` and show the result via
/// `window/showMessageRequest`.  Offers "Run Again" on both success and
/// failure, and additionally "Open File" on failure so the user can jump
/// straight to the test source.  Selecting "Run Again" re-executes the test
/// in the same task without returning to the client first.
async fn run_phpunit(
    client: &Client,
    filter: &str,
    root: Option<&std::path::Path>,
    file_uri: Option<&Url>,
) {
    let output = tokio::process::Command::new("vendor/bin/phpunit")
        .arg("--filter")
        .arg(filter)
        .current_dir(root.unwrap_or(std::path::Path::new(".")))
        .output()
        .await;

    let (success, message) = match output {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr);
            let last_line = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("(no output)")
                .to_string();
            let ok = out.status.success();
            let msg = if ok {
                format!("✓ {filter}: {last_line}")
            } else {
                format!("✗ {filter}: {last_line}")
            };
            (ok, msg)
        }
        Err(e) => (
            false,
            format!("php-lsp.runTest: failed to spawn phpunit — {e}"),
        ),
    };

    let msg_type = if success {
        MessageType::INFO
    } else {
        MessageType::ERROR
    };
    let mut actions = vec![MessageActionItem {
        title: "Run Again".to_string(),
        properties: Default::default(),
    }];
    if !success && file_uri.is_some() {
        actions.push(MessageActionItem {
            title: "Open File".to_string(),
            properties: Default::default(),
        });
    }

    let chosen = client
        .show_message_request(msg_type, message, Some(actions))
        .await;

    match chosen {
        Ok(Some(ref action)) if action.title == "Run Again" => {
            // Re-run once; result shown as a plain message to avoid infinite recursion.
            let output2 = tokio::process::Command::new("vendor/bin/phpunit")
                .arg("--filter")
                .arg(filter)
                .current_dir(root.unwrap_or(std::path::Path::new(".")))
                .output()
                .await;
            let msg2 = match output2 {
                Ok(out) => {
                    let text = String::from_utf8_lossy(&out.stdout).into_owned()
                        + &String::from_utf8_lossy(&out.stderr);
                    let last_line = text
                        .lines()
                        .rev()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("(no output)")
                        .to_string();
                    if out.status.success() {
                        format!("✓ {filter}: {last_line}")
                    } else {
                        format!("✗ {filter}: {last_line}")
                    }
                }
                Err(e) => format!("php-lsp.runTest: failed to spawn phpunit — {e}"),
            };
            client.show_message(MessageType::INFO, msg2).await;
        }
        Ok(Some(ref action)) if action.title == "Open File" => {
            if let Some(uri) = file_uri {
                client
                    .show_document(ShowDocumentParams {
                        uri: uri.clone(),
                        external: Some(false),
                        take_focus: Some(true),
                        selection: None,
                    })
                    .await
                    .ok();
            }
        }
        _ => {}
    }
}

/// Ask all connected clients to re-request semantic tokens, code lenses, inlay hints,
/// and diagnostics. Called after bulk index operations so that previously-opened editors
/// immediately pick up the newly indexed symbol information.
async fn send_refresh_requests(client: &Client) {
    client.send_request::<SemanticTokensRefresh>(()).await.ok();
    client.send_request::<CodeLensRefresh>(()).await.ok();
    client
        .send_request::<InlayHintRefreshRequest>(())
        .await
        .ok();
    client
        .send_request::<WorkspaceDiagnosticRefresh>(())
        .await
        .ok();
    client
        .send_request::<InlineValueRefreshRequest>(())
        .await
        .ok();
}

/// Maximum number of PHP files indexed during a workspace scan.
/// Prevents excessive memory use on projects with very large vendor trees.
const MAX_INDEXED_FILES: usize = 50_000;

/// Recursively scan `root` for `*.php` files and add them to the document store.
/// Skips hidden directories (names starting with `.`) and any path whose string
/// representation contains a segment matching one of the `exclude_paths` patterns.
/// Returns the number of files indexed.
async fn scan_workspace(
    root: PathBuf,
    docs: Arc<DocumentStore>,
    exclude_paths: &[String],
) -> usize {
    let mut count = 0usize;
    let mut stack = vec![root];

    while let Some(dir) = stack.pop() {
        if count >= MAX_INDEXED_FILES {
            break;
        }
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if count >= MAX_INDEXED_FILES {
                break;
            }
            let path = entry.path();
            let path_str = path.to_string_lossy();
            // Check user-configured exclude patterns (simple substring/prefix match).
            if exclude_paths.iter().any(|pat| {
                let p = pat.trim_end_matches('*').trim_end_matches('/');
                path_str.contains(p)
            }) {
                continue;
            }
            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // Skip hidden directories; vendor is indexed unless excluded above.
                if !name.starts_with('.') {
                    stack.push(path);
                }
            } else if file_type.is_file()
                && path.extension().is_some_and(|e| e == "php")
                && let Ok(uri) = Url::from_file_path(&path)
                && let Ok(text) = tokio::fs::read_to_string(&path).await
            {
                docs.index(uri, &text);
                count += 1;
            }
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{Position, Range, Url};

    // DiagnosticsConfig::from_value tests
    #[test]
    fn diagnostics_config_defaults_all_enabled() {
        let cfg = DiagnosticsConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.undefined_variables);
        assert!(cfg.undefined_functions);
        assert!(cfg.undefined_classes);
        assert!(cfg.arity_errors);
        assert!(cfg.type_errors);
        assert!(cfg.deprecated_calls);
        assert!(cfg.duplicate_declarations);
    }

    #[test]
    fn diagnostics_config_from_empty_object_uses_defaults() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!({}));
        assert!(cfg.enabled);
        assert!(cfg.undefined_variables);
    }

    #[test]
    fn diagnostics_config_from_non_object_uses_defaults() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!(null));
        assert!(cfg.enabled);
    }

    #[test]
    fn diagnostics_config_can_disable_individual_flags() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!({
            "enabled": true,
            "undefinedVariables": false,
            "undefinedFunctions": false,
            "undefinedClasses": true,
            "arityErrors": false,
            "typeErrors": true,
            "deprecatedCalls": false,
            "duplicateDeclarations": true,
        }));
        assert!(cfg.enabled);
        assert!(!cfg.undefined_variables);
        assert!(!cfg.undefined_functions);
        assert!(cfg.undefined_classes);
        assert!(!cfg.arity_errors);
        assert!(cfg.type_errors);
        assert!(!cfg.deprecated_calls);
        assert!(cfg.duplicate_declarations);
    }

    #[test]
    fn diagnostics_config_master_switch_disables_all() {
        let cfg = DiagnosticsConfig::from_value(&serde_json::json!({"enabled": false}));
        assert!(!cfg.enabled);
        // Other flags still have their default values
        assert!(cfg.undefined_variables);
    }

    // LspConfig::from_value tests
    #[test]
    fn lsp_config_default_is_empty() {
        let cfg = LspConfig::default();
        assert!(cfg.php_version.is_none());
        assert!(cfg.exclude_paths.is_empty());
        assert!(cfg.diagnostics.enabled);
    }

    #[test]
    fn lsp_config_parses_php_version() {
        let cfg = LspConfig::from_value(&serde_json::json!({"phpVersion": "8.2"}));
        assert_eq!(cfg.php_version.as_deref(), Some("8.2"));
    }

    #[test]
    fn lsp_config_parses_exclude_paths() {
        let cfg = LspConfig::from_value(&serde_json::json!({
            "excludePaths": ["cache/*", "generated/*"]
        }));
        assert_eq!(cfg.exclude_paths, vec!["cache/*", "generated/*"]);
    }

    #[test]
    fn lsp_config_parses_diagnostics_section() {
        let cfg = LspConfig::from_value(&serde_json::json!({
            "diagnostics": {"enabled": false}
        }));
        assert!(!cfg.diagnostics.enabled);
    }

    #[test]
    fn lsp_config_ignores_missing_fields() {
        let cfg = LspConfig::from_value(&serde_json::json!({}));
        assert!(cfg.php_version.is_none());
        assert!(cfg.exclude_paths.is_empty());
    }

    // find_use_insert_line tests
    #[test]
    fn find_use_insert_line_after_php_open_tag() {
        let src = "<?php\nfunction foo() {}";
        assert_eq!(find_use_insert_line(src), 1);
    }

    #[test]
    fn find_use_insert_line_after_existing_use() {
        let src = "<?php\nuse Foo\\Bar;\nuse Baz\\Qux;\nclass Impl {}";
        assert_eq!(find_use_insert_line(src), 3);
    }

    #[test]
    fn find_use_insert_line_after_namespace() {
        let src = "<?php\nnamespace App\\Services;\nclass Service {}";
        assert_eq!(find_use_insert_line(src), 2);
    }

    #[test]
    fn find_use_insert_line_after_namespace_and_use() {
        let src = "<?php\nnamespace App;\nuse Foo\\Bar;\nclass Impl {}";
        assert_eq!(find_use_insert_line(src), 3);
    }

    #[test]
    fn find_use_insert_line_empty_file() {
        assert_eq!(find_use_insert_line(""), 0);
    }

    // is_after_arrow tests
    #[test]
    fn is_after_arrow_with_method_call() {
        let src = "<?php\n$obj->method();\n";
        // Position after `->m` i.e. on `method` — character 6 (after `$obj->`)
        let pos = Position { line: 1, character: 6 };
        assert!(is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_without_arrow() {
        let src = "<?php\n$obj->method();\n";
        // Position on `$obj` — not after arrow
        let pos = Position { line: 1, character: 1 };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_on_standalone_identifier() {
        let src = "<?php\nfunction greet() {}\n";
        let pos = Position { line: 1, character: 10 };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_out_of_bounds_line() {
        let src = "<?php\n$x = 1;\n";
        let pos = Position { line: 99, character: 0 };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_at_start_of_property() {
        let src = "<?php\n$this->name;\n";
        // `name` starts at character 7 (after `$this->`)
        let pos = Position { line: 1, character: 7 };
        assert!(is_after_arrow(src, pos));
    }

    // php_file_op tests
    #[test]
    fn php_file_op_matches_php_files() {
        let op = php_file_op();
        assert_eq!(op.filters.len(), 1);
        let filter = &op.filters[0];
        assert_eq!(filter.scheme.as_deref(), Some("file"));
        assert_eq!(filter.pattern.glob, "**/*.php");
    }

    // defer_actions tests
    #[test]
    fn defer_actions_strips_edit_and_adds_data() {
        let uri = Url::parse("file:///test.php").unwrap();
        let range = Range {
            start: Position { line: 0, character: 0 },
            end: Position { line: 0, character: 5 },
        };
        let actions = vec![CodeActionOrCommand::CodeAction(CodeAction {
            title: "My Action".to_string(),
            kind: Some(CodeActionKind::REFACTOR),
            edit: Some(WorkspaceEdit::default()),
            data: None,
            ..Default::default()
        })];
        let deferred = defer_actions(actions, "test_kind", &uri, range);
        assert_eq!(deferred.len(), 1);
        if let CodeActionOrCommand::CodeAction(ca) = &deferred[0] {
            assert!(ca.edit.is_none(), "edit should be stripped");
            assert!(ca.data.is_some(), "data payload should be set");
            let data = ca.data.as_ref().unwrap();
            assert_eq!(data["php_lsp_resolve"], "test_kind");
            assert_eq!(data["uri"], uri.to_string());
        } else {
            panic!("expected CodeAction");
        }
    }

    // build_use_import_edit tests
    #[test]
    fn build_use_import_edit_inserts_after_php_tag() {
        let src = "<?php\nclass Foo {}";
        let uri = Url::parse("file:///test.php").unwrap();
        let edit = build_use_import_edit(src, &uri, "App\\Services\\Bar");
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, "use App\\Services\\Bar;\n");
        assert_eq!(edits[0].range.start.line, 1);
    }

    #[test]
    fn build_use_import_edit_inserts_after_existing_use() {
        let src = "<?php\nuse Foo\\Bar;\nclass Impl {}";
        let uri = Url::parse("file:///test.php").unwrap();
        let edit = build_use_import_edit(src, &uri, "Baz\\Qux");
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        assert_eq!(edits[0].range.start.line, 2);
        assert_eq!(edits[0].new_text, "use Baz\\Qux;\n");
    }
}
