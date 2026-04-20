use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::notification::Progress as ProgressNotification;
use tower_lsp::lsp_types::request::{
    CodeLensRefresh, InlayHintRefreshRequest, InlineValueRefreshRequest, SemanticTokensRefresh,
    WorkDoneProgressCreate, WorkspaceDiagnosticRefresh,
};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, async_trait};

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};

use crate::ast::{ParsedDoc, str_offset};
use crate::autoload::Psr4Map;
use crate::call_hierarchy::{incoming_calls, outgoing_calls, prepare_call_hierarchy};
use crate::code_lens::code_lenses;
use crate::completion::{CompletionCtx, filtered_completions_at};
use crate::declaration::{goto_declaration, goto_declaration_from_index};
use crate::definition::{find_declaration_range, find_in_indexes, goto_definition};
use crate::diagnostics::parse_document;
use crate::document_highlight::document_highlights;
use crate::document_link::document_links;
use crate::document_store::DocumentStore;
use crate::extract_action::extract_variable_actions;
use crate::extract_constant_action::extract_constant_actions;
use crate::extract_method_action::extract_method_actions;
use crate::file_rename::{use_edits_for_delete, use_edits_for_rename};
use crate::folding::folding_ranges;
use crate::formatting::{format_document, format_range};
use crate::generate_action::{generate_constructor_actions, generate_getters_setters_actions};
use crate::hover::{docs_for_symbol_from_index, hover_info, signature_for_symbol_from_index};
use crate::implement_action::implement_missing_actions;
use crate::implementation::{find_implementations, find_implementations_from_index};
use crate::inlay_hints::inlay_hints;
use crate::inline_action::inline_variable_actions;
use crate::inline_value::inline_values_in_range;
use crate::moniker::moniker_at;
use crate::on_type_format::on_type_format;
use crate::organize_imports::organize_imports_action;
use crate::phpdoc_action::phpdoc_actions;
use crate::phpstorm_meta::PhpStormMeta;
use crate::promote_action::promote_constructor_actions;
use crate::references::{SymbolKind, find_references, find_references_codebase};
use crate::rename::{prepare_rename, rename, rename_property, rename_variable};
use crate::selection_range::selection_ranges;
use crate::semantic_diagnostics::{
    deprecated_call_diagnostics, duplicate_declaration_diagnostics, index_file_references,
    semantic_diagnostics, semantic_diagnostics_no_rebuild,
};
use crate::semantic_tokens::{
    compute_token_delta, legend, semantic_tokens, semantic_tokens_range, token_hash,
};
use crate::signature_help::signature_help;
use crate::symbols::{document_symbols, resolve_workspace_symbol, workspace_symbols_from_index};
use crate::type_action::add_return_type_actions;
use crate::type_definition::{goto_type_definition, goto_type_definition_from_index};
use crate::type_hierarchy::{
    prepare_type_hierarchy_from_index, subtypes_of_from_index, supertypes_of_from_index,
};
use crate::use_import::{build_use_import_edit, find_fqn_for_class};
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
#[derive(Debug, Clone)]
pub struct LspConfig {
    /// PHP version string, e.g. `"8.1"`.  Set explicitly via `initializationOptions`
    /// or auto-detected from `composer.json` / the `php` binary at startup.
    pub php_version: Option<String>,
    /// Glob patterns for paths to exclude from workspace indexing.
    pub exclude_paths: Vec<String>,
    /// Per-category diagnostic toggles.
    pub diagnostics: DiagnosticsConfig,
    /// Maximum number of background-indexed files kept in memory (default: 1000).
    /// Lower this to reduce memory usage on large projects.
    pub max_indexed_files: usize,
}

impl Default for LspConfig {
    fn default() -> Self {
        LspConfig {
            php_version: None,
            exclude_paths: vec![],
            diagnostics: DiagnosticsConfig::default(),
            max_indexed_files: 1_000,
        }
    }
}

impl LspConfig {
    fn from_value(v: &serde_json::Value) -> Self {
        let mut cfg = LspConfig::default();
        if let Some(ver) = v.get("phpVersion").and_then(|x| x.as_str())
            && crate::autoload::is_valid_php_version(ver)
        {
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
        if let Some(n) = v
            .get("maxIndexedFiles")
            .and_then(|x| x.as_u64())
            .map(|x| x as usize)
        {
            cfg.max_indexed_files = n;
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
    codebase: Arc<mir_codebase::Codebase>,
    /// Set to `true` once the post-scan reference-indexing pass completes.
    /// `find_references_codebase` is only used when this is `true`.
    ref_index_ready: Arc<AtomicBool>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        let codebase = mir_codebase::Codebase::new();
        mir_analyzer::stubs::load_stubs(&codebase);
        Backend {
            client,
            docs: Arc::new(DocumentStore::new()),
            root_paths: Arc::new(RwLock::new(Vec::new())),
            psr4: Arc::new(RwLock::new(Psr4Map::empty())),
            meta: Arc::new(RwLock::new(PhpStormMeta::default())),
            config: Arc::new(RwLock::new(LspConfig::default())),
            codebase: Arc::new(codebase),
            ref_index_ready: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Run the definition collector for a single file, updating the persistent codebase.
    fn collect_definitions_for(&self, uri: &Url, doc: &ParsedDoc) {
        collect_into_codebase(&self.codebase, uri, doc);
    }

    /// Look up the import map for a file from the persistent codebase.
    fn file_imports(&self, uri: &Url) -> std::collections::HashMap<String, String> {
        self.codebase
            .file_imports
            .get(uri.as_str())
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    /// Resolve the PHP version to use. See `autoload::resolve_php_version_from_roots`
    /// for the full priority order.
    fn resolve_php_version(&self, explicit: Option<&str>) -> (String, &'static str) {
        let roots = self.root_paths.read().unwrap().clone();
        crate::autoload::resolve_php_version_from_roots(&roots, explicit)
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
        {
            let opts = params.initialization_options.as_ref();
            let mut cfg = opts.map(LspConfig::from_value).unwrap_or_default();
            // Warn if the client supplied an unrecognised phpVersion.
            if let Some(ver) = opts
                .and_then(|o| o.get("phpVersion"))
                .and_then(|v| v.as_str())
                && !crate::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: unsupported phpVersion {ver:?} — valid values: {}",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }
            // Resolve the PHP version and log what was chosen and why.
            let (ver, source) = self.resolve_php_version(cfg.php_version.as_deref());
            self.client
                .log_message(
                    tower_lsp::lsp_types::MessageType::INFO,
                    format!("php-lsp: using PHP {ver} ({source})"),
                )
                .await;
            // Show a visible warning when auto-detection yields a version outside
            // our supported range (e.g. a legacy project with ">=5.6" in composer.json).
            // TODO: instead of storing and using the unsupported version, consider clamping
            // it to the nearest supported version so analysis stays meaningful.
            if source != "set by editor" && !crate::autoload::is_valid_php_version(&ver) {
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range ({}); \
                             analysis may be inaccurate",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }
            cfg.php_version = Some(ver);
            self.docs.set_max_indexed(cfg.max_indexed_files);
            *self.config.write().unwrap() = cfg;
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
                register_options: Some(serde_json::json!({
                    "watchers": [{"globPattern": "**/*.php"}]
                })),
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
            let codebase = Arc::clone(&self.codebase);
            let ref_index_ready = Arc::clone(&self.ref_index_ready);
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
                    total += scan_workspace(
                        root,
                        Arc::clone(&docs),
                        &exclude_paths,
                        Arc::clone(&codebase),
                    )
                    .await;
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

                // Phase 3: build the reference index in the background so that
                // find_references_codebase can serve O(k) lookups instead of
                // scanning every file's AST. Runs after the progress notification
                // so the editor considers indexing "done" while this completes.
                build_reference_index(docs, codebase, ref_index_ready).await;
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
            let mut cfg = LspConfig::from_value(&value);
            if let Some(ver) = value.get("phpVersion").and_then(|v| v.as_str())
                && !crate::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: unsupported phpVersion {ver:?} — valid values: {}",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }
            // Resolve the PHP version and log what was chosen and why.
            let (ver, source) = self.resolve_php_version(cfg.php_version.as_deref());
            self.client
                .log_message(
                    tower_lsp::lsp_types::MessageType::INFO,
                    format!("php-lsp: using PHP {ver} ({source})"),
                )
                .await;
            // TODO: instead of storing and using the unsupported version, consider clamping
            // it to the nearest supported version so analysis stays meaningful.
            if source != "set by editor" && !crate::autoload::is_valid_php_version(&ver) {
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range ({}); \
                             analysis may be inaccurate",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }
            cfg.php_version = Some(ver);
            self.docs.set_max_indexed(cfg.max_indexed_files);
            *self.config.write().unwrap() = cfg;
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
                let cb = Arc::clone(&self.codebase);
                tokio::spawn(async move {
                    scan_workspace(path_clone, docs, &ex, cb).await;
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

        let codebase = Arc::clone(&self.codebase);
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();
        let php_version = self.config.read().unwrap().php_version.clone();
        let uri_clone = uri.clone();

        // Parse and run semantic analysis together in a blocking thread.
        // semantic_diagnostics handles remove → collect → finalize → analyze,
        // so definitions are never doubled even if the workspace scan already
        // indexed this file.
        let diag_cfg_inner = diag_cfg.clone();
        let (doc, parse_diags, sem_diags) = tokio::task::spawn_blocking(move || {
            let (doc, parse_diags) = parse_document(&text);
            let sem_diags = semantic_diagnostics(
                &uri_clone,
                &doc,
                &codebase,
                &diag_cfg_inner,
                php_version.as_deref(),
            );
            (doc, parse_diags, sem_diags)
        })
        .await
        .unwrap_or_else(|_| (ParsedDoc::default(), vec![], vec![]));

        self.docs
            .apply_parse(&uri, doc, parse_diags.clone(), version);
        let stored_source = self.docs.get(&uri).unwrap_or_default();
        let doc2 = self.docs.get_doc(&uri);
        let mut all_diags = parse_diags;
        if let Some(ref d) = doc2 {
            let dup_diags = duplicate_declaration_diagnostics(&stored_source, d, &diag_cfg);
            all_diags.extend(dup_diags);
        }
        all_diags.extend(sem_diags.clone());
        self.docs.set_sem_diagnostics(&uri, sem_diags);
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
        let codebase = Arc::clone(&self.codebase);
        let ref_index_ready = Arc::clone(&self.ref_index_ready);
        let diag_cfg = self.config.read().unwrap().diagnostics.clone();
        let php_version = self.config.read().unwrap().php_version.clone();
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
                    // semantic_diagnostics handles remove → collect → finalize → analyze
                    // as one unit, keeping the codebase consistent and ensuring any
                    // future collector-phase issues from mir-analyzer are surfaced.
                    let sem_diags = semantic_diagnostics(
                        &uri,
                        &d,
                        &codebase,
                        &diag_cfg,
                        php_version.as_deref(),
                    );
                    // Cache so code_action can read them without rerunning the rebuild.
                    docs.set_sem_diagnostics(&uri, sem_diags.clone());
                    all_diags.extend(sem_diags);
                    // Reference index requires a finalized codebase; semantic_diagnostics
                    // already called finalize() above.
                    if ref_index_ready.load(Ordering::Acquire) {
                        index_file_references(&uri, &d, &codebase);
                    }
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
                        // Parse first to collect into codebase, then index (which drops ParsedDoc).
                        let (doc, _diags) = parse_document(&text);
                        self.codebase.remove_file_definitions(change.uri.as_str());
                        self.collect_definitions_for(&change.uri, &doc);
                        self.codebase.finalize();
                        if self.ref_index_ready.load(Ordering::Acquire) {
                            index_file_references(&change.uri, &doc, &self.codebase);
                        }
                        self.docs.index(change.uri.clone(), &text);
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
        let imports = self.file_imports(uri);
        let ctx = CompletionCtx {
            source: Some(&source),
            position: Some(position),
            meta: meta_opt,
            doc_uri: Some(uri),
            file_imports: Some(&imports),
        };
        Ok(Some(CompletionResponse::Array(filtered_completions_at(
            &doc,
            &other_docs,
            trigger,
            &ctx,
        ))))
    }

    async fn completion_resolve(&self, mut item: CompletionItem) -> Result<CompletionItem> {
        if item.documentation.is_some() && item.detail.is_some() {
            return Ok(item);
        }
        // Strip trailing ':' from named-argument labels (e.g. "param:") before lookup.
        let name = item.label.trim_end_matches(':');
        let all_indexes = self.docs.all_indexes();
        if item.detail.is_none()
            && let Some(sig) = signature_for_symbol_from_index(name, &all_indexes)
        {
            item.detail = Some(sig);
        }
        if item.documentation.is_none()
            && let Some(md) = docs_for_symbol_from_index(name, &all_indexes)
        {
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
        // Search current file's ParsedDoc first (fast), then fall back to index search.
        let empty_other_docs: Vec<(Url, Arc<ParsedDoc>)> = vec![];
        if let Some(loc) = goto_definition(uri, &source, &doc, &empty_other_docs, position) {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }
        // Cross-file: use FileIndex (no disk I/O for background files).
        let other_indexes = self.docs.other_indexes(uri);
        if let Some(word) = crate::util::word_at(&source, position)
            && let Some(loc) = find_in_indexes(&word, &other_indexes)
        {
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
        let kind = if let Some(doc) = self.docs.get_doc(uri) {
            let stmts = &doc.program().stmts;
            if cursor_is_on_method_decl(doc.source(), stmts, position) {
                Some(SymbolKind::Method)
            } else {
                symbol_kind_at(&source, position, &word)
            }
        } else {
            symbol_kind_at(&source, position, &word)
        };
        let all_docs = self.docs.all_docs_for_scan();
        let include_declaration = params.context.include_declaration;

        // Fast path: use the pre-computed reference index once it is ready.
        // Falls back to the full AST scan for Method / None kinds, and whenever
        // the symbol is not found in the codebase (returns None).
        let locations = if self.ref_index_ready.load(Ordering::Acquire) {
            find_references_codebase(&word, &all_docs, include_declaration, kind, &self.codebase)
                .unwrap_or_else(|| find_references(&word, &all_docs, include_declaration, kind))
        } else {
            find_references(&word, &all_docs, include_declaration, kind)
        };

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
            Ok(Some(rename_variable(
                &word,
                &params.new_name,
                uri,
                &source,
                &doc,
                position,
            )))
        } else if is_after_arrow(&source, position) {
            let all_docs = self.docs.all_docs_for_scan();
            Ok(Some(rename_property(&word, &params.new_name, &all_docs)))
        } else {
            let all_docs = self.docs.all_docs_for_scan();
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
            let all_indexes = self.docs.all_indexes();
            if let Some(md) = docs_for_symbol_from_index(&name, &all_indexes) {
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
        let indexes = self.docs.all_indexes();
        let results = workspace_symbols_from_index(&params.query, &indexes);
        Ok(if results.is_empty() {
            None
        } else {
            Some(results)
        })
    }

    async fn symbol_resolve(&self, params: WorkspaceSymbol) -> Result<WorkspaceSymbol> {
        // For resolve, we need the full range from the ParsedDoc of open files.
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
        let result_id = token_hash(&tokens);
        self.docs
            .store_token_cache(uri, result_id.clone(), tokens.clone());
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: Some(result_id),
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
        let all_docs = self.docs.all_docs_for_scan();
        Ok(prepare_call_hierarchy(&word, &all_docs).map(|item| vec![item]))
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let all_docs = self.docs.all_docs_for_scan();
        let calls = incoming_calls(&params.item, &all_docs);
        Ok(if calls.is_empty() { None } else { Some(calls) })
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let all_docs = self.docs.all_docs_for_scan();
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
        let imports = self.file_imports(uri);
        let word = crate::util::word_at(&source, position).unwrap_or_default();
        let fqn = imports.get(&word).map(|s| s.as_str());
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.all_docs();
        let mut locs = find_implementations(&word, fqn, &open_docs);
        if locs.is_empty() {
            // Second pass: background files via FileIndex (line-only positions).
            let all_indexes = self.docs.all_indexes();
            locs = find_implementations_from_index(&word, fqn, &all_indexes);
        }
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
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.all_docs();
        if let Some(loc) = goto_declaration(&source, &open_docs, position) {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }
        // Second pass: background files via FileIndex (line-only positions).
        let all_indexes = self.docs.all_indexes();
        Ok(goto_declaration_from_index(&source, &all_indexes, position)
            .map(GotoDefinitionResponse::Scalar))
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
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.all_docs();
        if let Some(loc) = goto_type_definition(&source, &doc, &open_docs, position) {
            return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
        }
        // Second pass: background files via FileIndex (line-only positions).
        let all_indexes = self.docs.all_indexes();
        Ok(
            goto_type_definition_from_index(&source, &doc, &all_indexes, position)
                .map(GotoDefinitionResponse::Scalar),
        )
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let all_indexes = self.docs.all_indexes();
        Ok(
            prepare_type_hierarchy_from_index(&source, &all_indexes, position)
                .map(|item| vec![item]),
        )
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let all_indexes = self.docs.all_indexes();
        let result = supertypes_of_from_index(&params.item, &all_indexes);
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
        let all_indexes = self.docs.all_indexes();
        let result = subtypes_of_from_index(&params.item, &all_indexes);
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
        let all_docs = self.docs.all_docs_for_scan();
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
        let all_docs = self.docs.all_docs_for_scan();
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
                        start: Position {
                            line: 0,
                            character: 0,
                        },
                        end: Position {
                            line: 0,
                            character: 0,
                        },
                    },
                    new_text: stub,
                }],
            );
        }

        Ok(if changes.is_empty() {
            None
        } else {
            Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            })
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
        let all_docs = self.docs.all_docs_for_scan();
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
        let imports = self.file_imports(uri);
        Ok(moniker_at(&source, &doc, position, &imports).map(|m| vec![m]))
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
        let (diag_cfg, php_version) = {
            let cfg = self.config.read().unwrap();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };
        let sem_diags =
            semantic_diagnostics(uri, &doc, &self.codebase, &diag_cfg, php_version.as_deref());
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
        let all_parse_diags = self.docs.all_diagnostics();
        let (diag_cfg, php_version) = {
            let cfg = self.config.read().unwrap();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };

        // Build inheritance tables once for the entire workspace.
        // The persistent codebase already has all file definitions collected
        // incrementally via collect_into_codebase(). A single finalize() call
        // here is O(N); the old approach called finalize() per file → O(N²).
        self.codebase.finalize();

        let items: Vec<WorkspaceDocumentDiagnosticReport> = all_parse_diags
            .into_iter()
            .filter_map(|(uri, parse_diags, version)| {
                let doc = self.docs.get_doc(&uri)?;

                let source = doc.source().to_string();
                let sem_diags = semantic_diagnostics_no_rebuild(
                    &uri,
                    &doc,
                    &self.codebase,
                    &diag_cfg,
                    php_version.as_deref(),
                );
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

        // Reuse semantic diagnostics cached by did_open/did_change rather than
        // running a full codebase rebuild here — that rebuild takes write locks
        // which stall concurrent requests for ~1-2 s.
        let sem_diags = self.docs.get_sem_diagnostics(uri);

        // Build "Add use import" code actions for undefined class names in range
        let mut actions: Vec<CodeActionOrCommand> = Vec::new();
        for diag in &sem_diags {
            if diag.code != Some(NumberOrString::String("UndefinedClass".to_string())) {
                continue;
            }
            // Only act on diagnostics within the requested range
            if diag.range.start.line < params.range.start.line
                || diag.range.start.line > params.range.end.line
            {
                continue;
            }
            // Message format: "Class {name} does not exist"
            let class_name = diag
                .message
                .strip_prefix("Class ")
                .and_then(|s| s.strip_suffix(" does not exist"))
                .unwrap_or("")
                .trim();
            if class_name.is_empty() {
                continue;
            }

            // Find a class with this short name in other indexed documents
            for (_other_uri, other_doc) in &other_docs {
                if let Some(fqn) = find_fqn_for_class(other_doc, class_name) {
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
            implement_missing_actions(
                &source,
                &doc,
                &self.docs.doc_with_others(uri, Arc::clone(&doc)),
                params.range,
                uri,
                &self.file_imports(uri),
            ),
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
        actions.extend(defer_actions(
            promote_constructor_actions(&source, &doc, params.range, uri),
            "promote",
            uri,
            params.range,
        ));

        // Extract variable: cheap, keep eager.
        actions.extend(extract_variable_actions(&source, params.range, uri));
        actions.extend(extract_method_actions(&source, &doc, params.range, uri));
        actions.extend(extract_constant_actions(&source, params.range, uri));
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
                let imports = self.file_imports(&uri);
                implement_missing_actions(
                    &source,
                    &doc,
                    &self.docs.doc_with_others(&uri, Arc::clone(&doc)),
                    range,
                    &uri,
                    &imports,
                )
            }
            "constructor" => generate_constructor_actions(&source, &doc, range, &uri),
            "getters_setters" => generate_getters_setters_actions(&source, &doc, range, &uri),
            "return_type" => add_return_type_actions(&source, &doc, range, &uri),
            "promote" => promote_constructor_actions(&source, &doc, range, &uri),
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

/// Classify the symbol at `position` so `find_references` can use the right walker.
///
/// Heuristics (in priority order):
/// 1. Preceded by `->` or `?->` → `Method`
/// 2. Preceded by `::` → `Method` (static)
/// 3. Word starts with `$` → variable (returns `None`; variables are handled separately)
/// 4. First character is uppercase AND not preceded by `->` or `::` → `Class`
/// 5. Otherwise → `Function`
///
/// Falls back to `None` when the context cannot be determined.
fn symbol_kind_at(source: &str, position: Position, word: &str) -> Option<SymbolKind> {
    if word.starts_with('$') {
        return None; // variables handled elsewhere
    }
    let line = source.lines().nth(position.line as usize)?;
    let chars: Vec<char> = line.chars().collect();

    // Convert UTF-16 column to char index.
    let col = position.character as usize;
    let mut utf16_col = 0usize;
    let mut char_idx = 0usize;
    for ch in &chars {
        if utf16_col >= col {
            break;
        }
        utf16_col += ch.len_utf16();
        char_idx += 1;
    }

    // Walk left past identifier characters to find the first character before the word.
    let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
    while char_idx > 0 && is_word_char(chars[char_idx - 1]) {
        char_idx -= 1;
    }

    // Check for `->` or `?->`
    if char_idx >= 2 && chars[char_idx - 1] == '>' && chars[char_idx - 2] == '-' {
        return Some(SymbolKind::Method);
    }
    if char_idx >= 3
        && chars[char_idx - 1] == '>'
        && chars[char_idx - 2] == '-'
        && chars[char_idx - 3] == '?'
    {
        return Some(SymbolKind::Method);
    }

    // Check for `::`
    if char_idx >= 2 && chars[char_idx - 1] == ':' && chars[char_idx - 2] == ':' {
        return Some(SymbolKind::Method);
    }

    // If the word starts with an uppercase letter it is likely a class/interface/enum name.
    if word
        .chars()
        .next()
        .map(|c| c.is_uppercase())
        .unwrap_or(false)
    {
        return Some(SymbolKind::Class);
    }

    // Otherwise treat as a free function.
    Some(SymbolKind::Function)
}

/// Convert an LSP `Position` to a byte offset within `source`.
/// Returns `None` if the position is beyond the end of the source.
fn position_to_offset(source: &str, position: Position) -> Option<u32> {
    let mut byte_offset = 0usize;
    for (idx, line) in source.split('\n').enumerate() {
        if idx as u32 == position.line {
            // Strip trailing \r so CRLF lines don't affect column counting.
            let line_content = line.trim_end_matches('\r');
            let mut col = 0u32;
            for (byte_idx, ch) in line_content.char_indices() {
                if col >= position.character {
                    return Some((byte_offset + byte_idx) as u32);
                }
                col += ch.len_utf16() as u32;
            }
            return Some((byte_offset + line_content.len()) as u32);
        }
        byte_offset += line.len() + 1; // +1 for the '\n'
    }
    None
}

/// Returns `true` if the cursor is positioned on a method name inside a class,
/// interface, trait, or enum declaration in the AST.
///
/// This is a pre-pass used before the character-based `symbol_kind_at` heuristic
/// so that method *declarations* (`public function add() {}`) are classified as
/// `SymbolKind::Method` rather than falling through to `SymbolKind::Function`.
fn cursor_is_on_method_decl(source: &str, stmts: &[Stmt<'_, '_>], position: Position) -> bool {
    let Some(cursor) = position_to_offset(source, position) else {
        return false;
    };

    fn check(source: &str, stmts: &[Stmt<'_, '_>], cursor: u32) -> bool {
        for stmt in stmts {
            match &stmt.kind {
                StmtKind::Class(c) => {
                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Interface(i) => {
                    for member in i.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Trait(t) => {
                    for member in t.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Enum(e) => {
                    for member in e.members.iter() {
                        if let EnumMemberKind::Method(m) = &member.kind {
                            let start = str_offset(source, m.name);
                            let end = start + m.name.len() as u32;
                            if cursor >= start && cursor < end {
                                return true;
                            }
                        }
                    }
                }
                StmtKind::Namespace(ns) => {
                    if let NamespaceBody::Braced(inner) = &ns.body
                        && check(source, inner, cursor)
                    {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    check(source, stmts, cursor)
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

/// Run the definition collector for a single file against the persistent codebase.
fn collect_into_codebase(codebase: &mir_codebase::Codebase, uri: &Url, doc: &ParsedDoc) {
    let file: Arc<str> = Arc::from(uri.as_str());
    let source_map = php_rs_parser::source_map::SourceMap::new(doc.source());
    let collector = mir_analyzer::collector::DefinitionCollector::new(
        codebase,
        file,
        doc.source(),
        &source_map,
    );
    collector.collect(doc.program());
}

/// Maximum number of PHP files indexed during a workspace scan.
/// Prevents excessive memory use on projects with very large vendor trees.
const MAX_INDEXED_FILES: usize = 50_000;

/// Recursively scan `root` for `*.php` files and add them to the document store.
/// Skips hidden directories (names starting with `.`) and any path whose string
/// representation contains a segment matching one of the `exclude_paths` patterns.
/// Returns the number of files indexed.
///
/// Phase 1 — directory traversal: async, serial (I/O-bound; tokio handles it well).
/// Phase 2 — file reading + parsing: concurrent, bounded by available CPU cores.
async fn scan_workspace(
    root: PathBuf,
    docs: Arc<DocumentStore>,
    exclude_paths: &[String],
    codebase: Arc<mir_codebase::Codebase>,
) -> usize {
    // Phase 1: collect PHP file paths via async directory walk.
    let mut php_files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root];

    'walk: while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
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
            } else if file_type.is_file() && path.extension().is_some_and(|e| e == "php") {
                php_files.push(path);
                if php_files.len() >= MAX_INDEXED_FILES {
                    break 'walk;
                }
            }
        }
    }

    // Phase 2: read and parse files concurrently, bounded by available CPU cores.
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let sem = Arc::new(tokio::sync::Semaphore::new(parallelism));
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    for path in php_files {
        let permit = Arc::clone(&sem).acquire_owned().await.unwrap();
        let docs = Arc::clone(&docs);
        let codebase = Arc::clone(&codebase);
        let count = Arc::clone(&count);
        set.spawn(async move {
            let _permit = permit;
            let Ok(text) = tokio::fs::read_to_string(&path).await else {
                return;
            };
            let Ok(uri) = Url::from_file_path(&path) else {
                return;
            };
            tokio::task::spawn_blocking(move || {
                // Parse once: collect into codebase, then let docs.index()
                // extract the FileIndex and drop the ParsedDoc.
                let (doc, _diags) = parse_document(&text);
                collect_into_codebase(&codebase, &uri, &doc);
                docs.index(uri.clone(), &text);
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            })
            .await
            .ok();
        });
    }

    while set.join_next().await.is_some() {}

    count.load(std::sync::atomic::Ordering::Relaxed)
}

/// Phase 3 of workspace initialisation: run `StatementsAnalyzer` on every
/// indexed file to populate `codebase.symbol_reference_locations`.
///
/// This is deliberately run *after* the progress notification is sent so the
/// editor considers indexing finished while this background work completes.
/// Once done, `ref_index_ready` is set to `true` so the `references` handler
/// can switch to O(k) codebase lookups instead of scanning every AST.
async fn build_reference_index(
    docs: Arc<DocumentStore>,
    codebase: Arc<mir_codebase::Codebase>,
    ready: Arc<AtomicBool>,
) {
    // The codebase was already finalized at the end of the workspace scan
    // (Phase 2). Calling finalize() again here would race with concurrent
    // semantic_diagnostics calls that reset the finalized flag via
    // remove_file_definitions(), causing method-inheritance lookups to
    // transiently return None and silently drop those references from the index.
    let all_docs = docs.all_docs_for_scan();
    let parallelism = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let sem = Arc::new(tokio::sync::Semaphore::new(parallelism));
    let mut set: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

    for (uri, doc) in all_docs {
        let permit = Arc::clone(&sem).acquire_owned().await.unwrap();
        let codebase = Arc::clone(&codebase);
        set.spawn(async move {
            let _permit = permit;
            tokio::task::spawn_blocking(move || {
                index_file_references(&uri, &doc, &codebase);
            })
            .await
            .ok();
        });
    }

    while set.join_next().await.is_some() {}
    ready.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::use_import::find_use_insert_line;
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
        let cfg =
            LspConfig::from_value(&serde_json::json!({"phpVersion": crate::autoload::PHP_8_2}));
        assert_eq!(cfg.php_version.as_deref(), Some(crate::autoload::PHP_8_2));
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

    #[test]
    fn lsp_config_default_max_indexed_files() {
        let cfg = LspConfig::default();
        assert_eq!(cfg.max_indexed_files, 1_000);
    }

    #[test]
    fn lsp_config_parses_max_indexed_files() {
        let cfg = LspConfig::from_value(&serde_json::json!({"maxIndexedFiles": 500}));
        assert_eq!(cfg.max_indexed_files, 500);
    }

    #[test]
    fn lsp_config_ignores_invalid_max_indexed_files() {
        let cfg = LspConfig::from_value(&serde_json::json!({"maxIndexedFiles": "bad"}));
        assert_eq!(cfg.max_indexed_files, 1_000);
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
        let pos = Position {
            line: 1,
            character: 6,
        };
        assert!(is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_without_arrow() {
        let src = "<?php\n$obj->method();\n";
        // Position on `$obj` — not after arrow
        let pos = Position {
            line: 1,
            character: 1,
        };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_on_standalone_identifier() {
        let src = "<?php\nfunction greet() {}\n";
        let pos = Position {
            line: 1,
            character: 10,
        };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_out_of_bounds_line() {
        let src = "<?php\n$x = 1;\n";
        let pos = Position {
            line: 99,
            character: 0,
        };
        assert!(!is_after_arrow(src, pos));
    }

    #[test]
    fn is_after_arrow_at_start_of_property() {
        let src = "<?php\n$this->name;\n";
        // `name` starts at character 7 (after `$this->`)
        let pos = Position {
            line: 1,
            character: 7,
        };
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
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 5,
            },
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

    // Extraction logic for "Add use import" code action — matches IssueKind::UndefinedClass message format
    #[test]
    fn undefined_class_name_extracted_from_message() {
        let msg = "Class MyService does not exist";
        let name = msg
            .strip_prefix("Class ")
            .and_then(|s| s.strip_suffix(" does not exist"))
            .unwrap_or("")
            .trim();
        assert_eq!(name, "MyService");
    }

    #[test]
    fn undefined_function_message_not_matched_by_extraction() {
        // UndefinedFunction message format must NOT match the UndefinedClass extraction,
        // ensuring code action is not offered for undefined functions.
        let msg = "Function myHelper() is not defined";
        let name = msg
            .strip_prefix("Class ")
            .and_then(|s| s.strip_suffix(" does not exist"))
            .unwrap_or("")
            .trim();
        assert!(
            name.is_empty(),
            "function diagnostic should not extract a class name"
        );
    }

    // ── position_to_offset ───────────────────────────────────────────────────

    #[test]
    fn position_to_offset_first_line() {
        let src = "<?php\nfoo();";
        // Character 0 → byte 0.
        assert_eq!(
            position_to_offset(
                src,
                Position {
                    line: 0,
                    character: 0
                }
            ),
            Some(0)
        );
        // Character 4 → byte 4 (last char 'p' of "<?php").
        assert_eq!(
            position_to_offset(
                src,
                Position {
                    line: 0,
                    character: 4
                }
            ),
            Some(4)
        );
        // Character 5 is past the end of "<?php" (5 chars) — clamps to line_content.len().
        assert_eq!(
            position_to_offset(
                src,
                Position {
                    line: 0,
                    character: 5
                }
            ),
            Some(5)
        );
    }

    #[test]
    fn position_to_offset_second_line() {
        let src = "<?php\nfoo();";
        // Start of line 1 is byte 6 (after "<?php\n").
        assert_eq!(
            position_to_offset(
                src,
                Position {
                    line: 1,
                    character: 0
                }
            ),
            Some(6)
        );
        // "foo" ends at character 3 → byte 9.
        assert_eq!(
            position_to_offset(
                src,
                Position {
                    line: 1,
                    character: 3
                }
            ),
            Some(9)
        );
    }

    #[test]
    fn position_to_offset_line_boundary_returns_none() {
        // A source with exactly one line has only line 0; line 1 must return None.
        let src = "<?php";
        assert_eq!(
            position_to_offset(
                src,
                Position {
                    line: 1,
                    character: 0
                }
            ),
            None
        );
        assert_eq!(
            position_to_offset(
                src,
                Position {
                    line: 5,
                    character: 0
                }
            ),
            None
        );
    }

    // ── cursor_is_on_method_decl ─────────────────────────────────────────────

    #[test]
    fn cursor_on_method_decl_name_returns_true() {
        // "    public function add() {}" — "add" is cols 20-22 on line 2.
        // Use doc.source() so str_offset uses pointer arithmetic (production path).
        let doc = ParsedDoc::parse("<?php\nclass C {\n    public function add() {}\n}".to_string());
        let source = doc.source();
        let stmts = &doc.program().stmts;
        // All three characters of "add" must match.
        for col in 20u32..=22 {
            assert!(
                cursor_is_on_method_decl(
                    source,
                    stmts,
                    Position {
                        line: 2,
                        character: col
                    }
                ),
                "expected true at col {col}"
            );
        }
        // One before and one after must not match.
        assert!(!cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 19
            }
        ));
        assert!(!cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 23
            }
        ));
    }

    #[test]
    fn cursor_on_free_function_decl_returns_false() {
        // "add" at col 9 on line 1 is a free function — not a method.
        let doc = ParsedDoc::parse("<?php\nfunction add() {}".to_string());
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(!cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 1,
                character: 9
            }
        ));
    }

    #[test]
    fn cursor_on_method_call_site_returns_false() {
        // "$c->add()" — "add" at col 4 on line 3 is a call site, not a declaration.
        let doc = ParsedDoc::parse(
            "<?php\nclass C { public function add() {} }\n$c = new C();\n$c->add();".to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(!cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 3,
                character: 4
            }
        ));
    }

    #[test]
    fn cursor_on_interface_method_decl_returns_true() {
        // "    public function add(): void;" — "add" starts at col 20 on line 2.
        let doc = ParsedDoc::parse(
            "<?php\ninterface I {\n    public function add(): void;\n}".to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 20
            }
        ));
    }

    #[test]
    fn cursor_on_trait_method_decl_returns_true() {
        // "    public function add() {}" — "add" starts at col 20 on line 2.
        let doc = ParsedDoc::parse("<?php\ntrait T {\n    public function add() {}\n}".to_string());
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 20
            }
        ));
    }

    #[test]
    fn cursor_on_enum_method_decl_returns_true() {
        // "    public function label(): string {}" — "label" starts at col 20 on line 2.
        let doc = ParsedDoc::parse(
            "<?php\nenum Status {\n    public function label(): string { return 'x'; }\n}"
                .to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(cursor_is_on_method_decl(
            source,
            stmts,
            Position {
                line: 2,
                character: 20
            }
        ));
    }

    #[test]
    fn cursor_on_method_decl_in_unbraced_namespace_returns_true() {
        // Unbraced (Simple) namespace: the class is a top-level sibling of the
        // namespace statement, not nested inside it.
        //
        // Line 0: <?php
        // Line 1: namespace App;
        // Line 2: class C {
        // Line 3:     public function add() {}   ← "add" starts at col 20
        // Line 4: }
        let doc = ParsedDoc::parse(
            "<?php\nnamespace App;\nclass C {\n    public function add() {}\n}".to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(
            cursor_is_on_method_decl(
                source,
                stmts,
                Position {
                    line: 3,
                    character: 20
                }
            ),
            "method in unbraced namespace must be detected"
        );
    }

    #[test]
    fn cursor_on_method_decl_in_braced_namespace_returns_true() {
        // Braced namespace: the class is nested inside NamespaceBody::Braced.
        //
        // Line 0: <?php
        // Line 1: namespace App {
        // Line 2:     class C {
        // Line 3:         public function add() {}   ← "add" starts at col 24
        // Line 4:     }
        // Line 5: }
        let doc = ParsedDoc::parse(
            "<?php\nnamespace App {\n    class C {\n        public function add() {}\n    }\n}"
                .to_string(),
        );
        let source = doc.source();
        let stmts = &doc.program().stmts;
        assert!(
            cursor_is_on_method_decl(
                source,
                stmts,
                Position {
                    line: 3,
                    character: 24
                }
            ),
            "method in braced namespace must be detected"
        );
    }
}

#[cfg(test)]
mod integration {
    use super::Backend;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower_lsp::{LspService, Server};

    /// Encode a JSON value as an LSP-framed message.
    fn frame(msg: &serde_json::Value) -> Vec<u8> {
        let body = serde_json::to_string(msg).unwrap();
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
    }

    /// Read one LSP-framed response from `reader`.
    async fn read_msg(reader: &mut (impl AsyncReadExt + Unpin)) -> serde_json::Value {
        // Read headers until \r\n\r\n
        let mut header_buf = Vec::new();
        loop {
            let b = reader.read_u8().await.expect("read byte");
            header_buf.push(b);
            if header_buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let header_str = std::str::from_utf8(&header_buf).unwrap();
        let content_length: usize = header_str
            .lines()
            .find(|l| l.to_lowercase().starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse().ok())
            .expect("Content-Length header");
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body).await.expect("read body");
        serde_json::from_slice(&body).expect("parse JSON")
    }

    /// A minimal LSP test client backed by in-memory duplex streams.
    struct TestClient {
        write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
        read: tokio::io::ReadHalf<tokio::io::DuplexStream>,
        next_id: u64,
    }

    impl TestClient {
        fn new(
            write: tokio::io::WriteHalf<tokio::io::DuplexStream>,
            read: tokio::io::ReadHalf<tokio::io::DuplexStream>,
        ) -> Self {
            TestClient {
                write,
                read,
                next_id: 1,
            }
        }

        async fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
            let id = self.next_id;
            self.next_id += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            });
            self.write.write_all(&frame(&msg)).await.unwrap();
            // Read responses, skipping notifications (no "id" field), until we get our response
            loop {
                let resp = read_msg(&mut self.read).await;
                if resp.get("id") == Some(&serde_json::json!(id)) {
                    return resp;
                }
                // It's a notification (e.g. window/logMessage) — skip it
            }
        }

        /// Send a request with no params (the "params" key is omitted entirely).
        async fn request_no_params(&mut self, method: &str) -> serde_json::Value {
            let id = self.next_id;
            self.next_id += 1;
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
            });
            self.write.write_all(&frame(&msg)).await.unwrap();
            loop {
                let resp = read_msg(&mut self.read).await;
                if resp.get("id") == Some(&serde_json::json!(id)) {
                    return resp;
                }
            }
        }

        async fn notify(&mut self, method: &str, params: serde_json::Value) {
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            });
            self.write.write_all(&frame(&msg)).await.unwrap();
        }

        /// Read messages until a notification with the given method arrives (5 s timeout).
        async fn read_notification(&mut self, method: &str) -> serde_json::Value {
            tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
                loop {
                    let msg = read_msg(&mut self.read).await;
                    if msg.get("method") == Some(&serde_json::json!(method)) {
                        return msg;
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {method} notification"))
        }

        /// Read `textDocument/publishDiagnostics` notifications until one arrives for `uri`.
        async fn read_diagnostics_for(&mut self, uri: &str) -> serde_json::Value {
            let uri_val = serde_json::json!(uri);
            tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
                loop {
                    let msg = read_msg(&mut self.read).await;
                    if msg.get("method")
                        == Some(&serde_json::json!("textDocument/publishDiagnostics"))
                        && msg["params"]["uri"] == uri_val
                    {
                        return msg;
                    }
                }
            })
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for publishDiagnostics for {uri}"))
        }
    }

    fn start_server() -> TestClient {
        let (client_stream, server_stream) = tokio::io::duplex(1 << 20);
        let (server_read, server_write) = tokio::io::split(server_stream);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (service, socket) = LspService::new(Backend::new);
        tokio::spawn(Server::new(server_read, server_write, socket).serve(service));
        TestClient::new(client_write, client_read)
    }

    async fn initialize(client: &mut TestClient) -> serde_json::Value {
        let resp = client
            .request(
                "initialize",
                serde_json::json!({
                    "processId": null,
                    "rootUri": null,
                    "capabilities": {
                        "textDocument": {
                            "hover": { "contentFormat": ["markdown", "plaintext"] },
                            "completion": { "completionItem": { "snippetSupport": true } }
                        }
                    }
                }),
            )
            .await;
        // Send initialized notification (required by LSP spec)
        client.notify("initialized", serde_json::json!({})).await;
        resp
    }

    #[tokio::test]
    async fn initialize_returns_server_capabilities() {
        let mut client = start_server();
        let resp = initialize(&mut client).await;
        assert!(
            resp["error"].is_null(),
            "initialize should not error: {:?}",
            resp
        );
        let caps = &resp["result"]["capabilities"];
        assert!(caps.is_object(), "expected capabilities object");
        // Check a few key capabilities are advertised
        assert!(
            caps["hoverProvider"].as_bool().unwrap_or(false) || caps["hoverProvider"].is_object(),
            "hoverProvider should be enabled"
        );
        assert!(
            caps["textDocumentSync"].is_object() || caps["textDocumentSync"].is_number(),
            "textDocumentSync should be set"
        );
    }

    #[tokio::test]
    async fn hover_on_opened_document() {
        let mut client = start_server();
        initialize(&mut client).await;

        // Open a document
        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///test.php",
                        "languageId": "php",
                        "version": 1,
                        "text": "<?php\nfunction greet(string $name): string { return $name; }\n"
                    }
                }),
            )
            .await;

        // Give the async parser a moment to run
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Request hover on `greet` (line 1, char 10)
        let resp = client
            .request(
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": "file:///test.php" },
                    "position": { "line": 1, "character": 10 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "hover should not error: {:?}",
            resp
        );
        assert!(
            !resp["result"].is_null(),
            "hover on a known function must return a result, got null"
        );
        let value = resp["result"]["contents"]["value"]
            .as_str()
            .unwrap_or_default();
        assert!(
            value.contains("greet"),
            "hover must show function signature containing 'greet', got: {}",
            value
        );
    }

    #[tokio::test]
    async fn completion_after_initialize() {
        let mut client = start_server();
        initialize(&mut client).await;

        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///comp.php",
                        "languageId": "php",
                        "version": 1,
                        "text": "<?php\n"
                    }
                }),
            )
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        let resp = client
            .request(
                "textDocument/completion",
                serde_json::json!({
                    "textDocument": { "uri": "file:///comp.php" },
                    "position": { "line": 1, "character": 0 },
                    "context": { "triggerKind": 1 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "completion should not error: {:?}",
            resp
        );
        // result should be an array or completion list object
        let result = &resp["result"];
        assert!(
            result.is_array() || result.get("items").is_some() || result.is_null(),
            "unexpected completion result shape: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn did_change_updates_document() {
        let mut client = start_server();
        initialize(&mut client).await;

        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///change.php",
                        "languageId": "php",
                        "version": 1,
                        "text": "<?php\n"
                    }
                }),
            )
            .await;

        // Change the document
        client
            .notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": "file:///change.php", "version": 2 },
                    "contentChanges": [{ "text": "<?php\nfunction updated() {}\n" }]
                }),
            )
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Hover on `updated` — confirms the new content was applied
        let resp = client
            .request(
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": "file:///change.php" },
                    "position": { "line": 1, "character": 10 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "hover after change should not error"
        );
    }

    /// Gap 1: variable type from one method body must not appear in hover for the same
    /// variable name in a different method body (scope pollution via flat TypeMap).
    #[tokio::test]
    async fn hover_variable_type_is_scoped_to_enclosing_method() {
        // $result = new Widget() in methodA; $result = new Invoice() in methodB.
        // Hovering $result while inside methodB must show Invoice, not Widget.
        let src = concat!(
            "<?php\n",
            "class Widget {}\n",
            "class Invoice {}\n",
            "class Service {\n",
            "    public function methodA(): void { $result = new Widget(); }\n",
            "    public function methodB(): void { $result = new Invoice(); }\n",
            "}\n",
        );
        // Line 5 = "    public function methodB(): void { $result = new Invoice(); }"
        // "$result" starts at col 38 inside methodB
        let mut client = start_server();
        initialize(&mut client).await;
        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///scope_test.php",
                        "languageId": "php",
                        "version": 1,
                        "text": src
                    }
                }),
            )
            .await;
        // Wait for publishDiagnostics — guarantees the async parser has finished
        // and the document is fully indexed before we send the hover request.
        client.read_diagnostics_for("file:///scope_test.php").await;

        let resp = client
            .request(
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": "file:///scope_test.php" },
                    "position": { "line": 5, "character": 40 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "hover should not error: {:?}",
            resp
        );
        assert!(
            !resp["result"].is_null(),
            "expected hover result, got null — document may not have been parsed yet"
        );
        let value = resp["result"]["contents"]["value"]
            .as_str()
            .unwrap_or_default();
        assert!(
            !value.contains("Widget"),
            "Widget from methodA must not appear in methodB hover, got: {}",
            value
        );
        assert!(
            value.contains("Invoice"),
            "Invoice from methodB should appear, got: {}",
            value
        );
    }

    /// Gap 2: hovering a method call site `$obj->method()` must show the signature
    /// from the receiver's resolved class, not the first class with that method name.
    #[tokio::test]
    async fn hover_method_call_resolves_receiver_class() {
        // Both Mailer and Queue have `process()` with different signatures.
        // Hovering on $mailer->process() must show Mailer::process, not Queue::process.
        let src = concat!(
            "<?php\n",
            "class Mailer { public function process(string $to): bool {} }\n",
            "class Queue  { public function process(int $id): void {} }\n",
            "$mailer = new Mailer();\n",
            "$mailer->process('');\n",
        );
        // Line 4 = "$mailer->process('');" — "process" starts at col 9
        let mut client = start_server();
        initialize(&mut client).await;
        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///method_hover.php",
                        "languageId": "php",
                        "version": 1,
                        "text": src
                    }
                }),
            )
            .await;
        // Wait for publishDiagnostics to confirm the document is fully parsed.
        client
            .read_diagnostics_for("file:///method_hover.php")
            .await;

        let resp = client
            .request(
                "textDocument/hover",
                serde_json::json!({
                    "textDocument": { "uri": "file:///method_hover.php" },
                    "position": { "line": 4, "character": 12 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "hover should not error: {:?}",
            resp
        );
        assert!(
            !resp["result"].is_null(),
            "expected hover result on method call, got null"
        );
        let value = resp["result"]["contents"]["value"]
            .as_str()
            .unwrap_or_default();
        assert!(
            value.contains("Mailer"),
            "hover should show Mailer::process, got: {}",
            value
        );
        assert!(
            !value.contains("int $id"),
            "must NOT show Queue::process params, got: {}",
            value
        );
    }

    /// Regression test for issue #125: cursor on a method *declaration* must
    /// return method references, not free-function references with the same name.
    #[tokio::test]
    async fn references_on_method_decl_returns_method_refs_not_function_refs() {
        // Line 0: <?php
        // Line 1: function add() {}          ← free function declaration
        // Line 2: class C {
        // Line 3:     public function add() {} ← method declaration — cursor here
        // Line 4: }
        // Line 5: add();                     ← free function call
        // Line 6: $c->add();                 ← method call
        let src = "<?php\nfunction add() {}\nclass C {\n    public function add() {}\n}\nadd();\n$c->add();";

        let mut client = start_server();
        initialize(&mut client).await;

        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///refs_test.php",
                        "languageId": "php",
                        "version": 1,
                        "text": src
                    }
                }),
            )
            .await;

        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

        // Cursor on "add" in "    public function add() {}" — line 3, character 20.
        let resp = client
            .request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": "file:///refs_test.php" },
                    "position": { "line": 3, "character": 20 },
                    "context": { "includeDeclaration": true }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "references should not error: {:?}",
            resp
        );

        let locs = resp["result"]
            .as_array()
            .expect("expected array of locations");
        let lines: Vec<u32> = locs
            .iter()
            .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
            .collect();

        assert!(
            lines.contains(&3),
            "method declaration (line 3) must be included, got: {:?}",
            lines
        );
        assert!(
            lines.contains(&6),
            "method call (line 6) must be included, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&1),
            "free-function declaration (line 1) must be excluded, got: {:?}",
            lines
        );
        assert!(
            !lines.contains(&5),
            "free-function call (line 5) must be excluded, got: {:?}",
            lines
        );

        // Same cursor, includeDeclaration: false — only the method call should appear.
        let resp2 = client
            .request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": "file:///refs_test.php" },
                    "position": { "line": 3, "character": 20 },
                    "context": { "includeDeclaration": false }
                }),
            )
            .await;

        assert!(
            resp2["error"].is_null(),
            "references (no decl) should not error: {:?}",
            resp2
        );

        let lines2: Vec<u32> = resp2["result"]
            .as_array()
            .expect("expected array of locations")
            .iter()
            .map(|l| l["range"]["start"]["line"].as_u64().unwrap() as u32)
            .collect();

        assert!(
            lines2.contains(&6),
            "method call (line 6) must be included when includeDeclaration=false, got: {:?}",
            lines2
        );
        assert!(
            !lines2.contains(&3),
            "method declaration (line 3) must be excluded when includeDeclaration=false, got: {:?}",
            lines2
        );
    }

    /// Multi-file variant of the regression test for issue #125.
    ///
    /// When the cursor is on a method *declaration* the server must scan all
    /// indexed files for method references and must not bleed into free-function
    /// references in a different file that share the same name.
    ///
    /// Document layout
    /// ───────────────
    /// file:///a.php   — contains the class with the method declaration (cursor file)
    ///   Line 0: <?php
    ///   Line 1: class C {
    ///   Line 2:     public function add() {}   ← cursor here (character 20)
    ///   Line 3: }
    ///
    /// file:///b.php   — contains a free function with the same name AND a method call
    ///   Line 0: <?php
    ///   Line 1: function add() {}              ← free-function decl — must be excluded
    ///   Line 2: add();                         ← free-function call — must be excluded
    ///   Line 3: $c->add();                     ← method call — must be included
    #[tokio::test]
    async fn references_on_method_decl_excludes_cross_file_free_function() {
        let src_a = "<?php\nclass C {\n    public function add() {}\n}";
        let src_b = "<?php\nfunction add() {}\nadd();\n$c->add();";

        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(&mut client, "file:///a.php", src_a).await;
        open_doc(&mut client, "file:///b.php", src_b).await;

        // Cursor on "add" in "    public function add() {}" — line 2, character 20.
        let resp = client
            .request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": "file:///a.php" },
                    "position": { "line": 2, "character": 20 },
                    "context": { "includeDeclaration": true }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "references should not error: {:?}",
            resp
        );

        let locs = resp["result"]
            .as_array()
            .expect("expected array of locations");

        // Helper: collect (uri, line) pairs so failures are easy to read.
        let hits: Vec<(&str, u32)> = locs
            .iter()
            .map(|l| {
                (
                    l["uri"].as_str().unwrap(),
                    l["range"]["start"]["line"].as_u64().unwrap() as u32,
                )
            })
            .collect();

        assert!(
            hits.contains(&("file:///a.php", 2)),
            "method declaration (a.php line 2) must be included, got: {:?}",
            hits
        );
        assert!(
            hits.contains(&("file:///b.php", 3)),
            "method call (b.php line 3) must be included, got: {:?}",
            hits
        );
        assert!(
            !hits.contains(&("file:///b.php", 1)),
            "free-function declaration (b.php line 1) must be excluded, got: {:?}",
            hits
        );
        assert!(
            !hits.contains(&("file:///b.php", 2)),
            "free-function call (b.php line 2) must be excluded, got: {:?}",
            hits
        );
    }

    #[tokio::test]
    async fn shutdown_responds_correctly() {
        let mut client = start_server();
        initialize(&mut client).await;

        let resp = client.request_no_params("shutdown").await;

        assert!(
            resp["error"].is_null(),
            "shutdown should not error: {:?}",
            resp
        );
        assert!(resp["result"].is_null(), "shutdown result should be null");
    }

    /// Open a document and wait for the async parser to finish.
    async fn open_doc(client: &mut TestClient, uri: &str, text: &str) {
        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "php",
                        "version": 1,
                        "text": text
                    }
                }),
            )
            .await;
        // Parser is debounced 100 ms; give it a little extra.
        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
    }

    // ── go-to-definition ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn definition_returns_location_for_function() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///def.php",
            "<?php\nfunction greet(string $name): string { return $name; }\ngreet('world');\n",
        )
        .await;

        // Cursor on `greet` in the call on line 2, char 0.
        let resp = client
            .request(
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": "file:///def.php" },
                    "position": { "line": 2, "character": 1 }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "definition error: {:?}", resp);
        // Result is either a Location object or an array of Locations.
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "expected a definition location, got null"
        );
        let loc = if result.is_array() {
            result[0].clone()
        } else {
            result.clone()
        };
        assert_eq!(
            loc["uri"].as_str().unwrap(),
            "file:///def.php",
            "definition should point to same file"
        );
        // `function greet` — `function ` is 9 chars, so `greet` starts at char 9.
        assert_eq!(
            loc["range"]["start"]["line"].as_u64().unwrap(),
            1,
            "definition should point to line 1 (the declaration)"
        );
        assert_eq!(
            loc["range"]["start"]["character"].as_u64().unwrap(),
            9,
            "definition should point to the function name at char 9, not the 'function' keyword"
        );
    }

    #[tokio::test]
    async fn definition_for_class_returns_location() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///cls.php",
            "<?php\nclass Dog {}\n$d = new Dog();\n",
        )
        .await;

        // Cursor on `Dog` in `new Dog()` — line 2, char 9 ('D').
        let resp = client
            .request(
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": "file:///cls.php" },
                    "position": { "line": 2, "character": 9 }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "definition error: {:?}", resp);
        let result = &resp["result"];
        assert!(!result.is_null(), "expected a location for class Dog");
        let loc = if result.is_array() {
            result[0].clone()
        } else {
            result.clone()
        };
        // `class Dog {}` — `class ` is 6 chars, so `Dog` starts at char 6.
        assert_eq!(
            loc["range"]["start"]["line"].as_u64().unwrap(),
            1,
            "Dog declared on line 1"
        );
        assert_eq!(
            loc["range"]["start"]["character"].as_u64().unwrap(),
            6,
            "Dog name starts at char 6, not at the 'class' keyword"
        );
    }

    /// Cross-file goto-definition exercises `find_in_indexes`: the symbol is
    /// defined in file A (open, so its FileIndex is populated) but the cursor
    /// is in file B where the symbol is used.  The single-file first pass on B
    /// finds nothing, so the handler falls through to `find_in_indexes` which
    /// searches all other files' FileIndex entries.
    #[tokio::test]
    async fn definition_cross_file_uses_find_in_indexes() {
        let mut client = start_server();
        initialize(&mut client).await;

        // File A defines Greeter.
        open_doc(
            &mut client,
            "file:///greeter.php",
            "<?php\nclass Greeter {}\n",
        )
        .await;
        // File B uses Greeter — Greeter is NOT defined here.
        open_doc(
            &mut client,
            "file:///user.php",
            "<?php\n$g = new Greeter();\n",
        )
        .await;

        // Cursor on `Greeter` in `new Greeter()` — line 1, char 9 ('G').
        let resp = client
            .request(
                "textDocument/definition",
                serde_json::json!({
                    "textDocument": { "uri": "file:///user.php" },
                    "position": { "line": 1, "character": 9 }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "definition error: {:?}", resp);
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "expected a cross-file definition for Greeter, got null"
        );
        let loc = if result.is_array() {
            result[0].clone()
        } else {
            result.clone()
        };
        assert_eq!(
            loc["uri"].as_str().unwrap(),
            "file:///greeter.php",
            "definition must point to greeter.php"
        );
        assert_eq!(
            loc["range"]["start"]["line"].as_u64().unwrap(),
            1,
            "Greeter is declared on line 1 of greeter.php"
        );
    }

    // ── go-to-declaration ────────────────────────────────────────────────────

    #[tokio::test]
    async fn declaration_returns_location_for_abstract_method() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///abs.php",
            "<?php\nabstract class Animal {\n    abstract public function speak(): string;\n}\nclass Cat extends Animal {\n    public function speak(): string { return 'meow'; }\n}\n",
        )
        .await;

        // Cursor on concrete `speak` on line 5, char 20.
        let resp = client
            .request(
                "textDocument/declaration",
                serde_json::json!({
                    "textDocument": { "uri": "file:///abs.php" },
                    "position": { "line": 5, "character": 20 }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "declaration error: {:?}", resp);
        // go-to-declaration from a concrete override must return the abstract declaration.
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "expected a declaration location for concrete speak(), got null"
        );
        let loc = if result.is_array() {
            result[0].clone()
        } else {
            result.clone()
        };
        assert_eq!(loc["uri"].as_str().unwrap(), "file:///abs.php");
        // Abstract `speak` is on line 2: `    abstract public function speak()…`
        // `    abstract public function ` = 4+9+7+9 = 29 chars → char 29.
        assert_eq!(
            loc["range"]["start"]["line"].as_u64().unwrap(),
            2,
            "should point to the abstract declaration on line 2"
        );
        assert_eq!(
            loc["range"]["start"]["character"].as_u64().unwrap(),
            29,
            "should point to the method name, not the 'abstract' keyword"
        );
    }

    // ── find references ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn references_finds_all_usages_of_function() {
        let mut client = start_server();
        initialize(&mut client).await;

        // One declaration (line 1) + two call sites (lines 2, 3).
        open_doc(
            &mut client,
            "file:///refs.php",
            "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\nadd(3, 4);\n",
        )
        .await;

        // Cursor on `add` declaration — line 1, char 9.
        let resp = client
            .request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": "file:///refs.php" },
                    "position": { "line": 1, "character": 9 },
                    "context": { "includeDeclaration": true }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "references error: {:?}", resp);
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "references should return an array, got: {:?}",
            result
        );
        let locs = result.as_array().unwrap();
        // Must include the declaration (line 1) AND both call sites (lines 2, 3).
        assert_eq!(
            locs.len(),
            3,
            "expected 3 references (1 declaration + 2 calls), got: {:?}",
            locs
        );
        let lines: Vec<u64> = locs
            .iter()
            .map(|l| l["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert!(
            lines.contains(&1),
            "declaration on line 1 must be included with includeDeclaration=true, got lines: {:?}",
            lines
        );
        assert!(lines.contains(&2), "call on line 2 must be included");
        assert!(lines.contains(&3), "call on line 3 must be included");
    }

    #[tokio::test]
    async fn references_with_exclude_declaration() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///refs2.php",
            "<?php\nfunction sub(int $a, int $b): int { return $a - $b; }\nsub(10, 3);\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/references",
                serde_json::json!({
                    "textDocument": { "uri": "file:///refs2.php" },
                    "position": { "line": 1, "character": 9 },
                    "context": { "includeDeclaration": false }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "references error: {:?}", resp);
        let result = &resp["result"];
        // With includeDeclaration: false, the only result must be the call on line 2.
        assert!(
            result.is_array(),
            "expected an array of references, got: {:?}",
            result
        );
        let locs = result.as_array().unwrap();
        assert_eq!(
            locs.len(),
            1,
            "expected exactly 1 call-site reference (sub on line 2), got: {:?}",
            locs
        );
        assert_eq!(
            locs[0]["range"]["start"]["line"].as_u64().unwrap(),
            2,
            "call site should be on line 2, not the declaration line 1"
        );
        assert_eq!(
            locs[0]["range"]["start"]["character"].as_u64().unwrap(),
            0,
            "call starts at char 0"
        );
    }

    // ── go-to-type-definition ────────────────────────────────────────────────

    #[tokio::test]
    async fn type_definition_for_typed_variable() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///typedef.php",
            "<?php\nclass Point { public int $x; public int $y; }\n$p = new Point();\n$p->x;\n",
        )
        .await;

        // Cursor on `$p` in `$p->x` — line 3, char 1.
        let resp = client
            .request(
                "textDocument/typeDefinition",
                serde_json::json!({
                    "textDocument": { "uri": "file:///typedef.php" },
                    "position": { "line": 3, "character": 1 }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "typeDefinition error: {:?}", resp);
        // Type inference resolves `$p` to `Point`; result must be non-null.
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "expected typeDefinition to resolve $p to Point, got null"
        );
        let loc = if result.is_array() {
            result[0].clone()
        } else {
            result.clone()
        };
        // `class Point` — `class ` is 6 chars, `Point` starts at char 6 on line 1.
        assert_eq!(
            loc["range"]["start"]["line"].as_u64().unwrap(),
            1,
            "type definition should point to Point class on line 1"
        );
        assert_eq!(
            loc["range"]["start"]["character"].as_u64().unwrap(),
            6,
            "type definition should point to the class name, not the 'class' keyword"
        );
    }

    // ── implementation ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn implementation_finds_concrete_class() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///impl.php",
            "<?php\ninterface Drawable {\n    public function draw(): void;\n}\nclass Circle implements Drawable {\n    public function draw(): void {}\n}\n",
        )
        .await;

        // Cursor on `Drawable` interface — line 1, char 10.
        let resp = client
            .request(
                "textDocument/implementation",
                serde_json::json!({
                    "textDocument": { "uri": "file:///impl.php" },
                    "position": { "line": 1, "character": 10 }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "implementation error: {:?}", resp);
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "implementation must return an array: {:?}",
            result
        );
        let locs = result.as_array().unwrap();
        assert!(
            !locs.is_empty(),
            "expected at least one implementation (Circle)"
        );
        // `class Circle` — `class ` is 6 chars, `Circle` starts at char 6 on line 4.
        let circle = locs
            .iter()
            .find(|l| l["range"]["start"]["line"].as_u64() == Some(4))
            .expect("expected an implementation result on line 4 (class Circle)");
        assert_eq!(
            circle["range"]["start"]["character"].as_u64().unwrap(),
            6,
            "Circle class name should start at char 6, not the 'class' keyword"
        );
    }

    // ── signature help ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn signature_help_inside_function_call() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///sig.php",
            "<?php\nfunction multiply(int $a, int $b): int { return $a * $b; }\nmultiply(2, \n",
        )
        .await;

        // Cursor inside the argument list — line 2, char 11 (after the comma).
        let resp = client
            .request(
                "textDocument/signatureHelp",
                serde_json::json!({
                    "textDocument": { "uri": "file:///sig.php" },
                    "position": { "line": 2, "character": 11 }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "signatureHelp error: {:?}", resp);
        // Cursor is after the comma in `multiply(2, ` → second parameter (index 1).
        let result = &resp["result"];
        assert!(!result.is_null(), "expected signatureHelp result, got null");
        let sigs = result["signatures"]
            .as_array()
            .expect("signatures must be an array");
        assert!(!sigs.is_empty(), "expected at least one signature");
        assert_eq!(
            sigs[0]["label"].as_str().unwrap(),
            "multiply(int $a, int $b)",
            "signature label should show the full parameter list"
        );
        assert_eq!(
            result["activeParameter"].as_u64().unwrap(),
            1,
            "cursor after first comma → activeParameter should be 1"
        );
    }

    // ── document symbols ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn document_symbols_lists_functions_and_classes() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///syms.php",
            "<?php\nfunction hello(): void {}\nclass World {}\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/documentSymbol",
                serde_json::json!({
                    "textDocument": { "uri": "file:///syms.php" }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "documentSymbol error: {:?}", resp);
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "documentSymbol should return an array, got: {:?}",
            result
        );
        let syms = result.as_array().unwrap();
        assert!(
            syms.len() >= 2,
            "expected at least 2 symbols (hello, World), got {}",
            syms.len()
        );
        let names: Vec<&str> = syms.iter().filter_map(|s| s["name"].as_str()).collect();
        assert!(names.contains(&"hello"), "missing symbol 'hello'");
        assert!(names.contains(&"World"), "missing symbol 'World'");
    }

    // ── document highlight ────────────────────────────────────────────────────

    #[tokio::test]
    async fn document_highlight_marks_occurrences() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///hl.php",
            "<?php\nfunction run(): void {}\nrun();\nrun();\n",
        )
        .await;

        // Cursor on `run` declaration — line 1, char 9.
        let resp = client
            .request(
                "textDocument/documentHighlight",
                serde_json::json!({
                    "textDocument": { "uri": "file:///hl.php" },
                    "position": { "line": 1, "character": 9 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "documentHighlight error: {:?}",
            resp
        );
        // Declaration on line 1 + two calls on lines 2 and 3 = 3 highlights.
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "documentHighlight must return an array: {:?}",
            result
        );
        let highlights = result.as_array().unwrap();
        assert_eq!(
            highlights.len(),
            3,
            "expected 3 highlights (1 declaration + 2 calls), got: {:?}",
            highlights
        );
        let lines: Vec<u64> = highlights
            .iter()
            .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert!(
            lines.contains(&1),
            "declaration highlight missing on line 1"
        );
        assert!(lines.contains(&2), "call highlight missing on line 2");
        assert!(lines.contains(&3), "call highlight missing on line 3");
    }

    #[tokio::test]
    async fn document_highlight_variable_inside_enum_method() {
        // Regression: collect_in_fn_at previously had no arm for StmtKind::Enum,
        // so variables inside enum methods were never scoped correctly.
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///enum_hl.php",
            "<?php\nenum Status {\n    public function label($arg) { return $arg + 1; }\n}\n",
        )
        .await;

        // Cursor on `arg` of `$arg` param — line 2, char 27.
        let resp = client
            .request(
                "textDocument/documentHighlight",
                serde_json::json!({
                    "textDocument": { "uri": "file:///enum_hl.php" },
                    "position": { "line": 2, "character": 27 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "documentHighlight error: {:?}",
            resp
        );
        let highlights = resp["result"].as_array().expect("expected array");
        // param declaration + body usage = 2 highlights, both on line 2
        assert_eq!(
            highlights.len(),
            2,
            "expected 2 highlights (param + body ref in enum method), got: {:?}",
            highlights
        );
        let lines: Vec<u64> = highlights
            .iter()
            .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert!(
            lines.iter().all(|&l| l == 2),
            "both highlights must be on line 2 (inside enum method), got: {:?}",
            lines
        );
    }

    // ── inlay hints ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn inlay_hints_returned_for_function_call() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///hints.php",
            "<?php\nfunction divide(int $dividend, int $divisor): float { return $dividend / $divisor; }\ndivide(10, 2);\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/inlayHint",
                serde_json::json!({
                    "textDocument": { "uri": "file:///hints.php" },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end":   { "line": 3, "character": 0 }
                    }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "inlayHint error: {:?}", resp);
        // `divide(10, 2)` has two named params → expect two hints: `dividend:` and `divisor:`.
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "expected inlayHint array, got: {:?}",
            result
        );
        let hints = result.as_array().unwrap();
        assert_eq!(
            hints.len(),
            2,
            "expected 2 inlay hints (dividend and divisor), got: {:?}",
            hints
        );
        let labels: Vec<&str> = hints.iter().filter_map(|h| h["label"].as_str()).collect();
        assert!(
            labels.contains(&"dividend:"),
            "missing hint 'dividend:', got: {:?}",
            labels
        );
        assert!(
            labels.contains(&"divisor:"),
            "missing hint 'divisor:', got: {:?}",
            labels
        );
    }

    // ── rename ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn rename_function_produces_workspace_edit() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ren.php",
            "<?php\nfunction oldName(): void {}\noldName();\n",
        )
        .await;

        // Cursor on `oldName` declaration — line 1, char 9.
        let resp = client
            .request(
                "textDocument/rename",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ren.php" },
                    "position": { "line": 1, "character": 9 },
                    "newName": "newName"
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "rename error: {:?}", resp);
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "expected rename to produce a WorkspaceEdit, got null"
        );
        // WorkspaceEdit must have either `changes` or `documentChanges`.
        assert!(
            result.get("changes").is_some() || result.get("documentChanges").is_some(),
            "rename result should be a WorkspaceEdit: {:?}",
            result
        );
        // One declaration (line 1) + one call (line 2) = 2 edits in the same file.
        let file_edits = result["changes"]["file:///ren.php"]
            .as_array()
            .expect("expected edits for file:///ren.php");
        assert_eq!(
            file_edits.len(),
            2,
            "expected 2 edits (declaration + call), got: {:?}",
            file_edits
        );
        let edited_lines: Vec<u64> = file_edits
            .iter()
            .map(|e| e["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert!(
            edited_lines.contains(&1),
            "declaration on line 1 must be renamed"
        );
        assert!(
            edited_lines.contains(&2),
            "call site on line 2 must be renamed"
        );
    }

    #[tokio::test]
    async fn rename_variable_inside_enum_method() {
        // Regression: collect_in_fn_at previously had no arm for StmtKind::Enum,
        // so renaming a variable inside an enum method produced zero edits.
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///enum_ren.php",
            "<?php\nenum Status {\n    public function label($arg) { return $arg + 1; }\n}\n",
        )
        .await;

        // Cursor on `arg` of `$arg` param — line 2, char 27.
        let resp = client
            .request(
                "textDocument/rename",
                serde_json::json!({
                    "textDocument": { "uri": "file:///enum_ren.php" },
                    "position": { "line": 2, "character": 27 },
                    "newName": "value"
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "rename error: {:?}", resp);
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "expected rename to produce a WorkspaceEdit, got null"
        );
        let file_edits = result["changes"]["file:///enum_ren.php"]
            .as_array()
            .expect("expected edits for file:///enum_ren.php");
        // param declaration + body usage = 2 edits, both on line 2
        assert_eq!(
            file_edits.len(),
            2,
            "expected 2 edits (param + body ref in enum method), got: {:?}",
            file_edits
        );
        let edited_lines: Vec<u64> = file_edits
            .iter()
            .map(|e| e["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert!(
            edited_lines.iter().all(|&l| l == 2),
            "both edits must be on line 2 (inside enum method), got: {:?}",
            edited_lines
        );
    }

    #[tokio::test]
    async fn document_highlight_enum_method_does_not_bleed_outer_scope() {
        // Regression guard: cursor on $arg inside enum method must NOT highlight
        // an outer $arg defined at file scope.
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///enum_scope.php",
            "<?php\n$arg = 0;\nenum Status {\n    public function label($arg) { return $arg + 1; }\n}\n",
        )
        .await;

        // Cursor on `arg` of `$arg` param — line 3, char 27.
        let resp = client
            .request(
                "textDocument/documentHighlight",
                serde_json::json!({
                    "textDocument": { "uri": "file:///enum_scope.php" },
                    "position": { "line": 3, "character": 27 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "documentHighlight error: {:?}",
            resp
        );
        let highlights = resp["result"].as_array().expect("expected array");
        // Only the param + body reference inside the enum method (line 3); outer $arg on line 1 must be excluded.
        assert_eq!(
            highlights.len(),
            2,
            "expected exactly 2 highlights (param + body ref), got: {:?}",
            highlights
        );
        let lines: Vec<u64> = highlights
            .iter()
            .map(|h| h["range"]["start"]["line"].as_u64().unwrap())
            .collect();
        assert!(
            lines.iter().all(|&l| l == 3),
            "all highlights must be on line 3 (inside enum method), outer $arg must not appear: {:?}",
            lines
        );
    }

    // ── folding ranges ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn folding_ranges_returned_for_class() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///fold.php",
            "<?php\nclass Folded {\n    public function method(): void {\n        // body\n    }\n}\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/foldingRange",
                serde_json::json!({
                    "textDocument": { "uri": "file:///fold.php" }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "foldingRange error: {:?}", resp);
        // Class (lines 1–5) + method (lines 2–4) = 2 fold ranges.
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "foldingRange must return an array: {:?}",
            result
        );
        let ranges = result.as_array().unwrap();
        assert_eq!(
            ranges.len(),
            2,
            "expected 2 fold ranges (class + method), got: {:?}",
            ranges
        );
        let start_lines: Vec<u64> = ranges
            .iter()
            .map(|r| r["startLine"].as_u64().unwrap())
            .collect();
        assert!(
            start_lines.contains(&1),
            "missing class fold starting at line 1"
        );
        assert!(
            start_lines.contains(&2),
            "missing method fold starting at line 2"
        );
    }

    // ── semantic tokens ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn semantic_tokens_full_returned() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///tokens.php",
            "<?php\nfunction tokenized(int $x): int { return $x; }\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/semanticTokens/full",
                serde_json::json!({
                    "textDocument": { "uri": "file:///tokens.php" }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "semanticTokens/full error: {:?}",
            resp
        );
        // A file with a function and typed parameters must produce non-empty token data.
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "expected semanticTokens result, got null"
        );
        let data = result["data"].as_array().expect("data must be an array");
        assert!(
            !data.is_empty(),
            "expected non-empty semantic token data for a file with a typed function"
        );
    }

    // ── code lens ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn code_lens_returned_for_function() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///lens.php",
            "<?php\nfunction lensed(): void {}\nlensed();\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/codeLens",
                serde_json::json!({
                    "textDocument": { "uri": "file:///lens.php" }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "codeLens error: {:?}", resp);
        // `lensed` has 1 call site → expect a "1 references" lens on the declaration.
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "codeLens must return an array: {:?}",
            result
        );
        let lenses = result.as_array().unwrap();
        assert!(!lenses.is_empty(), "expected at least one code lens");
        let has_ref_lens = lenses.iter().any(|l| {
            l["command"]["title"]
                .as_str()
                .map(|t| t.contains("reference"))
                .unwrap_or(false)
        });
        assert!(
            has_ref_lens,
            "expected a reference-count lens, got: {:?}",
            lenses
        );
    }

    // ── selection range ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn selection_range_expands_from_position() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///sel.php",
            "<?php\nfunction select(int $x): int { return $x + 1; }\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/selectionRange",
                serde_json::json!({
                    "textDocument": { "uri": "file:///sel.php" },
                    "positions": [{ "line": 1, "character": 30 }]
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "selectionRange error: {:?}", resp);
        // Cursor is inside the function body — must return at least one range.
        // The outermost range must NOT use u32::MAX as the end character (Bug #2 fix).
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "selectionRange must return an array: {:?}",
            result
        );
        let items = result.as_array().unwrap();
        assert!(
            !items.is_empty(),
            "expected at least one selectionRange entry"
        );

        // Walk to the outermost parent and verify its end character is spec-compliant.
        let mut node = &items[0];
        loop {
            let end_char = node["range"]["end"]["character"].as_u64().unwrap_or(0);
            assert_ne!(
                end_char,
                u32::MAX as u64,
                "selectionRange end character must not be u32::MAX — use real line length"
            );
            if node["parent"].is_null() || !node["parent"].is_object() {
                break;
            }
            node = &node["parent"];
        }
    }

    // ── full probe (disabled; restore #[tokio::test] + run with --nocapture to inspect) ──

    #[allow(dead_code)]
    async fn probe_all_features() {
        macro_rules! dump {
            ($label:expr, $r:expr) => {
                eprintln!(
                    "\n=== {} ===\n{}",
                    $label,
                    serde_json::to_string_pretty(&$r["result"]).unwrap_or_default()
                );
            };
        }

        let mut client = start_server();
        initialize(&mut client).await;

        // ── definition ──────────────────────────────────────────────────────
        open_doc(
            &mut client,
            "file:///p_def.php",
            "<?php\nfunction greet(string $name): string { return $name; }\ngreet('world');\n",
        )
        .await;
        dump!("definition/function (cursor on call line 2 char 1)",
            client.request("textDocument/definition",
                serde_json::json!({"textDocument":{"uri":"file:///p_def.php"},"position":{"line":2,"character":1}})).await);

        // ── definition on cursor ON the declaration name ─────────────────────
        dump!("definition/function (cursor on decl line 1 char 9)",
            client.request("textDocument/definition",
                serde_json::json!({"textDocument":{"uri":"file:///p_def.php"},"position":{"line":1,"character":9}})).await);

        // ── references (now fixed) ───────────────────────────────────────────
        dump!("references includeDeclaration=true",
            client.request("textDocument/references",
                serde_json::json!({"textDocument":{"uri":"file:///p_def.php"},"position":{"line":1,"character":9},"context":{"includeDeclaration":true}})).await);

        dump!("references includeDeclaration=false",
            client.request("textDocument/references",
                serde_json::json!({"textDocument":{"uri":"file:///p_def.php"},"position":{"line":1,"character":9},"context":{"includeDeclaration":false}})).await);

        // ── document symbols ─────────────────────────────────────────────────
        open_doc(&mut client, "file:///p_syms.php",
            "<?php\nfunction hello(): void {}\nclass World {}\nenum Color { case Red; }\ninterface Runnable {}\n").await;
        dump!(
            "documentSymbol",
            client
                .request(
                    "textDocument/documentSymbol",
                    serde_json::json!({"textDocument":{"uri":"file:///p_syms.php"}})
                )
                .await
        );

        // ── type definition ──────────────────────────────────────────────────
        open_doc(
            &mut client,
            "file:///p_type.php",
            "<?php\nclass Point { public int $x; public int $y; }\n$p = new Point();\n$p->x;\n",
        )
        .await;
        dump!("typeDefinition ($p->x, cursor on $p line 3 char 1)",
            client.request("textDocument/typeDefinition",
                serde_json::json!({"textDocument":{"uri":"file:///p_type.php"},"position":{"line":3,"character":1}})).await);

        // ── declaration ──────────────────────────────────────────────────────
        open_doc(&mut client, "file:///p_decl.php",
            "<?php\nabstract class Animal {\n    abstract public function speak(): string;\n}\nclass Cat extends Animal {\n    public function speak(): string { return 'meow'; }\n}\n").await;
        dump!("declaration (concrete speak at line 5 char 20 -> abstract)",
            client.request("textDocument/declaration",
                serde_json::json!({"textDocument":{"uri":"file:///p_decl.php"},"position":{"line":5,"character":20}})).await);

        // ── implementation ───────────────────────────────────────────────────
        open_doc(&mut client, "file:///p_impl.php",
            "<?php\ninterface Drawable {\n    public function draw(): void;\n}\nclass Circle implements Drawable {\n    public function draw(): void {}\n}\nclass Square implements Drawable {\n    public function draw(): void {}\n}\n").await;
        dump!("implementation (Drawable interface line 1 char 10)",
            client.request("textDocument/implementation",
                serde_json::json!({"textDocument":{"uri":"file:///p_impl.php"},"position":{"line":1,"character":10}})).await);

        // ── signature help ───────────────────────────────────────────────────
        open_doc(&mut client, "file:///p_sig.php",
            "<?php\nfunction divide(int $dividend, int $divisor): float { return $dividend / $divisor; }\ndivide(10, \n").await;
        dump!("signatureHelp (inside second arg)",
            client.request("textDocument/signatureHelp",
                serde_json::json!({"textDocument":{"uri":"file:///p_sig.php"},"position":{"line":2,"character":10}})).await);

        // ── document highlight ───────────────────────────────────────────────
        open_doc(
            &mut client,
            "file:///p_hl.php",
            "<?php\nfunction run(): void {}\nrun();\nrun();\n",
        )
        .await;
        dump!("documentHighlight (run decl line 1 char 9)",
            client.request("textDocument/documentHighlight",
                serde_json::json!({"textDocument":{"uri":"file:///p_hl.php"},"position":{"line":1,"character":9}})).await);

        // ── rename ───────────────────────────────────────────────────────────
        open_doc(
            &mut client,
            "file:///p_ren.php",
            "<?php\nfunction oldName(): void {}\noldName();\noldName();\n",
        )
        .await;
        dump!("rename (oldName -> newName, decl at line 1 char 9)",
            client.request("textDocument/rename",
                serde_json::json!({"textDocument":{"uri":"file:///p_ren.php"},"position":{"line":1,"character":9},"newName":"newName"})).await);

        // ── folding ranges ────────────────────────────────────────────────────
        open_doc(&mut client, "file:///p_fold.php",
            "<?php\nclass Folded {\n    public function method(): void {\n        // body\n    }\n}\n").await;
        dump!(
            "foldingRange",
            client
                .request(
                    "textDocument/foldingRange",
                    serde_json::json!({"textDocument":{"uri":"file:///p_fold.php"}})
                )
                .await
        );

        // ── semantic tokens ───────────────────────────────────────────────────
        open_doc(
            &mut client,
            "file:///p_tok.php",
            "<?php\nfunction tokenized(int $x): int { return $x; }\n",
        )
        .await;
        dump!(
            "semanticTokens/full (raw data)",
            client
                .request(
                    "textDocument/semanticTokens/full",
                    serde_json::json!({"textDocument":{"uri":"file:///p_tok.php"}})
                )
                .await
        );

        // ── code lens ────────────────────────────────────────────────────────
        open_doc(
            &mut client,
            "file:///p_lens.php",
            "<?php\nfunction lensed(): void {}\nlensed();\nlensed();\n",
        )
        .await;
        dump!(
            "codeLens",
            client
                .request(
                    "textDocument/codeLens",
                    serde_json::json!({"textDocument":{"uri":"file:///p_lens.php"}})
                )
                .await
        );

        // ── inlay hints ───────────────────────────────────────────────────────
        open_doc(&mut client, "file:///p_hints.php",
            "<?php\nfunction divide2(int $dividend, int $divisor): float { return $dividend / $divisor; }\ndivide2(10, 2);\n").await;
        dump!("inlayHint",
            client.request("textDocument/inlayHint",
                serde_json::json!({"textDocument":{"uri":"file:///p_hints.php"},"range":{"start":{"line":0,"character":0},"end":{"line":3,"character":0}}})).await);

        // ── selection range ───────────────────────────────────────────────────
        open_doc(
            &mut client,
            "file:///p_sel.php",
            "<?php\nfunction select(int $x): int { return $x + 1; }\n",
        )
        .await;
        dump!("selectionRange (cursor inside return expr)",
            client.request("textDocument/selectionRange",
                serde_json::json!({"textDocument":{"uri":"file:///p_sel.php"},"positions":[{"line":1,"character":38}]})).await);
    }

    #[tokio::test]
    async fn diagnostics_published_on_did_open_for_undefined_function() {
        let mut client = start_server();
        initialize(&mut client).await;

        // PHP with a clear undefined-function error that mir-analyzer detects.
        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///diag_test.php",
                        "languageId": "php",
                        "version": 1,
                        "text": "<?php\nnonexistent_function();\n"
                    }
                }),
            )
            .await;

        let notif = client.read_diagnostics_for("file:///diag_test.php").await;
        let diags = &notif["params"]["diagnostics"];
        let has_undefined_fn = diags
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|d| d["code"].as_str() == Some("UndefinedFunction"));
        assert!(
            has_undefined_fn,
            "expected an UndefinedFunction diagnostic in publishDiagnostics, got: {:?}",
            diags
        );
    }

    #[tokio::test]
    async fn diagnostics_published_on_did_change_for_undefined_function() {
        let mut client = start_server();
        initialize(&mut client).await;

        // Open a valid file first — consumes its publishDiagnostics notification.
        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///change_test.php",
                        "languageId": "php",
                        "version": 1,
                        "text": "<?php\n"
                    }
                }),
            )
            .await;
        client.read_diagnostics_for("file:///change_test.php").await;

        // Now change the file to introduce an undefined-function call.
        client
            .notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": "file:///change_test.php", "version": 2 },
                    "contentChanges": [{ "text": "<?php\nnonexistent_function();\n" }]
                }),
            )
            .await;

        let notif = client.read_diagnostics_for("file:///change_test.php").await;
        let diags = &notif["params"]["diagnostics"];
        let has_undefined_fn = diags
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|d| d["code"].as_str() == Some("UndefinedFunction"));
        assert!(
            has_undefined_fn,
            "expected an UndefinedFunction diagnostic after didChange, got: {:?}",
            diags
        );
    }

    #[tokio::test]
    async fn did_open_emits_diagnostic_for_undefined_class() {
        let mut client = start_server();
        initialize(&mut client).await;

        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///undef_class.php",
                        "languageId": "php",
                        "version": 1,
                        "text": "<?php\n$x = new UnknownClass();\n"
                    }
                }),
            )
            .await;

        let notif = client.read_diagnostics_for("file:///undef_class.php").await;
        let diags = &notif["params"]["diagnostics"];
        let has_undef_class = diags
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|d| d["code"].as_str() == Some("UndefinedClass"));
        assert!(
            has_undef_class,
            "expected UndefinedClass diagnostic on did_open, got: {:?}",
            diags
        );
    }

    #[tokio::test]
    async fn diagnostics_clear_when_code_is_fixed() {
        let mut client = start_server();
        initialize(&mut client).await;

        // Open broken file — expect a diagnostic.
        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": "file:///fix_test.php",
                        "languageId": "php",
                        "version": 1,
                        "text": "<?php\nnonexistent_function();\n"
                    }
                }),
            )
            .await;
        let notif = client.read_diagnostics_for("file:///fix_test.php").await;
        assert!(
            !notif["params"]["diagnostics"]
                .as_array()
                .unwrap_or(&vec![])
                .is_empty(),
            "expected at least one diagnostic for broken code, got: {:?}",
            notif["params"]["diagnostics"]
        );

        // Fix the file — diagnostics must clear to an empty array.
        client
            .notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": "file:///fix_test.php", "version": 2 },
                    "contentChanges": [{ "text": "<?php\n" }]
                }),
            )
            .await;
        let notif = client.read_diagnostics_for("file:///fix_test.php").await;
        let diags = notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&vec![])
            .clone();
        assert!(
            diags.is_empty(),
            "diagnostics must be empty after fixing the code, got: {:?}",
            diags
        );
    }

    // Reproduces the exact PHP from issue #170: broken code inside a class
    // method under a namespace. Verifies the fix works for the reporter's
    // actual use case, not just top-level statements.

    // Stripped-down variants to isolate where mir-analyzer stops detecting.
    const PHP_UNDEF_FN_TOP_LEVEL: &str = "<?php\nnonexistent_function();\n";
    const PHP_UNDEF_FN_IN_FUNCTION: &str =
        "<?php\nfunction f(): void {\n    nonexistent_function();\n}\n";
    const PHP_UNDEF_FN_IN_METHOD: &str = "<?php\nclass A {\n    public function f(): void {\n        nonexistent_function();\n    }\n}\n";
    const PHP_UNDEF_FN_IN_NAMESPACED_METHOD: &str = "<?php\nnamespace LspTest;\nclass Broken {\n    public function f(): void {\n        nonexistent_function();\n    }\n}\n";

    const ISSUE_170_PHP: &str = r#"<?php
namespace LspTest;

class Broken
{
    public int $count = 0;

    public function bump(): int
    {
        $this->count++;
        return $this->count;
    }

    public function obviouslyBroken(): int
    {
        nonexistent_function();
        $x = new UnknownClass();
        return 0;
    }
}
"#;

    async fn diags_for(client: &mut TestClient, uri: &str, text: &str) -> Vec<String> {
        client
            .notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": "php",
                        "version": 1,
                        "text": text
                    }
                }),
            )
            .await;
        let notif = client.read_diagnostics_for(uri).await;
        let empty = vec![];
        notif["params"]["diagnostics"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|d| d["code"].as_str().map(str::to_owned))
            .collect()
    }

    #[tokio::test]
    async fn mir_analyzer_scope_of_undefined_function_detection() {
        let mut client = start_server();
        initialize(&mut client).await;

        let top_level = diags_for(&mut client, "file:///scope1.php", PHP_UNDEF_FN_TOP_LEVEL).await;
        assert!(
            top_level.contains(&"UndefinedFunction".to_owned()),
            "top-level call: expected UndefinedFunction, got: {:?}",
            top_level
        );

        let in_function =
            diags_for(&mut client, "file:///scope2.php", PHP_UNDEF_FN_IN_FUNCTION).await;
        assert!(
            in_function.contains(&"UndefinedFunction".to_owned()),
            "call inside plain function: expected UndefinedFunction, got: {:?}",
            in_function
        );

        let in_method = diags_for(&mut client, "file:///scope3.php", PHP_UNDEF_FN_IN_METHOD).await;
        assert!(
            in_method.contains(&"UndefinedFunction".to_owned()),
            "call inside class method: expected UndefinedFunction, got: {:?}",
            in_method
        );

        let in_namespaced_method = diags_for(
            &mut client,
            "file:///scope4.php",
            PHP_UNDEF_FN_IN_NAMESPACED_METHOD,
        )
        .await;
        assert!(
            in_namespaced_method.contains(&"UndefinedFunction".to_owned()),
            "call inside namespaced class method: expected UndefinedFunction, got: {:?}",
            in_namespaced_method
        );
    }

    // The reporter's exact PHP from issue #170 — verifies that mir-analyzer
    // detects errors inside namespaced class method bodies.
    #[tokio::test]
    async fn issue_170_undefined_function_in_method_body_is_detected() {
        let mut client = start_server();
        initialize(&mut client).await;

        let codes = diags_for(&mut client, "file:///issue170.php", ISSUE_170_PHP).await;
        assert!(
            codes.contains(&"UndefinedFunction".to_owned()),
            "expected UndefinedFunction inside a namespaced class method, got: {:?}",
            codes
        );
        assert!(
            codes.contains(&"UndefinedClass".to_owned()),
            "expected UndefinedClass inside a namespaced class method, got: {:?}",
            codes
        );
    }

    // ── call hierarchy ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn call_hierarchy_prepare_returns_item() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ch.php",
            "<?php\nfunction callee(): void {}\ncallee();\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/prepareCallHierarchy",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ch.php" },
                    "position": { "line": 1, "character": 9 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "prepareCallHierarchy error: {:?}",
            resp
        );
        let result = &resp["result"];
        assert!(
            result.is_array(),
            "expected array result, got: {:?}",
            result
        );
        let items = result.as_array().unwrap();
        assert!(!items.is_empty(), "expected at least one CallHierarchyItem");
        assert_eq!(
            items[0]["name"].as_str().unwrap_or(""),
            "callee",
            "expected item name to be 'callee', got: {:?}",
            items[0]
        );
    }

    #[tokio::test]
    async fn call_hierarchy_incoming_calls_finds_caller() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ch_in.php",
            "<?php\nfunction callee(): void {}\nfunction caller(): void { callee(); }\n",
        )
        .await;

        let prep = client
            .request(
                "textDocument/prepareCallHierarchy",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ch_in.php" },
                    "position": { "line": 1, "character": 9 }
                }),
            )
            .await;

        let item = &prep["result"][0];
        assert!(item.is_object(), "need a prepared item to continue");

        let resp = client
            .request(
                "callHierarchy/incomingCalls",
                serde_json::json!({ "item": item }),
            )
            .await;

        assert!(resp["error"].is_null(), "incomingCalls error: {:?}", resp);
        let calls = resp["result"].as_array().expect("expected array");
        assert!(!calls.is_empty(), "expected at least one incoming call");
        assert!(
            calls
                .iter()
                .any(|c| c["from"]["name"].as_str() == Some("caller")),
            "expected 'caller' as incoming caller, got: {:?}",
            calls
        );
    }

    #[tokio::test]
    async fn call_hierarchy_outgoing_calls_finds_callee() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ch_out.php",
            "<?php\nfunction inner(): void {}\nfunction outer(): void { inner(); }\n",
        )
        .await;

        let prep = client
            .request(
                "textDocument/prepareCallHierarchy",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ch_out.php" },
                    "position": { "line": 2, "character": 9 }
                }),
            )
            .await;

        let item = &prep["result"][0];
        assert!(item.is_object(), "need a prepared item to continue");

        let resp = client
            .request(
                "callHierarchy/outgoingCalls",
                serde_json::json!({ "item": item }),
            )
            .await;

        assert!(resp["error"].is_null(), "outgoingCalls error: {:?}", resp);
        let calls = resp["result"].as_array().expect("expected array");
        assert!(!calls.is_empty(), "expected at least one outgoing call");
        assert!(
            calls
                .iter()
                .any(|c| c["to"]["name"].as_str() == Some("inner")),
            "expected 'inner' as outgoing callee, got: {:?}",
            calls
        );
    }

    // ── type hierarchy ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn type_hierarchy_prepare_returns_item() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(&mut client, "file:///th.php", "<?php\nclass MyClass {}\n").await;

        let resp = client
            .request(
                "textDocument/prepareTypeHierarchy",
                serde_json::json!({
                    "textDocument": { "uri": "file:///th.php" },
                    "position": { "line": 1, "character": 6 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "prepareTypeHierarchy error: {:?}",
            resp
        );
        let result = &resp["result"];
        assert!(result.is_array(), "expected array, got: {:?}", result);
        let items = result.as_array().unwrap();
        assert!(!items.is_empty(), "expected at least one TypeHierarchyItem");
        assert_eq!(
            items[0]["name"].as_str().unwrap_or(""),
            "MyClass",
            "expected item name 'MyClass', got: {:?}",
            items[0]
        );
    }

    #[tokio::test]
    async fn type_hierarchy_supertypes_finds_parent() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///th_super.php",
            "<?php\nclass ParentClass {}\nclass ChildClass extends ParentClass {}\n",
        )
        .await;

        let prep = client
            .request(
                "textDocument/prepareTypeHierarchy",
                serde_json::json!({
                    "textDocument": { "uri": "file:///th_super.php" },
                    "position": { "line": 2, "character": 6 }
                }),
            )
            .await;

        let item = &prep["result"][0];
        assert!(item.is_object(), "need a prepared item to continue");

        let resp = client
            .request(
                "typeHierarchy/supertypes",
                serde_json::json!({ "item": item }),
            )
            .await;

        assert!(resp["error"].is_null(), "supertypes error: {:?}", resp);
        let types = resp["result"].as_array().expect("expected array");
        assert!(!types.is_empty(), "expected parent in supertypes");
        assert!(
            types
                .iter()
                .any(|t| t["name"].as_str() == Some("ParentClass")),
            "expected ParentClass in supertypes, got: {:?}",
            types
        );
    }

    #[tokio::test]
    async fn type_hierarchy_subtypes_finds_child() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///th_sub.php",
            "<?php\ninterface Runnable {}\nclass Runner implements Runnable {}\n",
        )
        .await;

        let prep = client
            .request(
                "textDocument/prepareTypeHierarchy",
                serde_json::json!({
                    "textDocument": { "uri": "file:///th_sub.php" },
                    "position": { "line": 1, "character": 10 }
                }),
            )
            .await;

        let item = &prep["result"][0];
        assert!(item.is_object(), "need a prepared item to continue");

        let resp = client
            .request(
                "typeHierarchy/subtypes",
                serde_json::json!({ "item": item }),
            )
            .await;

        assert!(resp["error"].is_null(), "subtypes error: {:?}", resp);
        let types = resp["result"].as_array().expect("expected array");
        assert!(!types.is_empty(), "expected child in subtypes");
        assert!(
            types.iter().any(|t| t["name"].as_str() == Some("Runner")),
            "expected Runner in subtypes, got: {:?}",
            types
        );
    }

    // ── workspace symbols ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn workspace_symbols_returns_matching_items() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///wsym.php",
            "<?php\nclass FuzzyTarget {}\n",
        )
        .await;

        let resp = client
            .request(
                "workspace/symbol",
                serde_json::json!({ "query": "FuzzyTarget" }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "workspace/symbol error: {:?}",
            resp
        );
        let result = &resp["result"];
        assert!(result.is_array(), "expected array, got: {:?}", result);
        let items = result.as_array().unwrap();
        assert!(!items.is_empty(), "expected at least one symbol");
        assert!(
            items
                .iter()
                .any(|s| s["name"].as_str() == Some("FuzzyTarget")),
            "expected FuzzyTarget in results, got: {:?}",
            items
        );
    }

    // ── completion resolve ────────────────────────────────────────────────────

    #[tokio::test]
    async fn completion_resolve_returns_item() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///cresolve.php",
            "<?php\nfunction resolveMe(): void {}\nresolveM\n",
        )
        .await;

        let comp = client
            .request(
                "textDocument/completion",
                serde_json::json!({
                    "textDocument": { "uri": "file:///cresolve.php" },
                    "position": { "line": 2, "character": 8 }
                }),
            )
            .await;

        let items = match &comp["result"] {
            v if v.is_array() => v.as_array().unwrap().to_vec(),
            v if v["items"].is_array() => v["items"].as_array().unwrap().to_vec(),
            _ => vec![],
        };

        assert!(
            !items.is_empty(),
            "expected completions for 'resolveM' prefix, got: {:?}",
            comp["result"]
        );

        // Find the resolveMe item specifically so the assertion is deterministic.
        let resolve_me = items
            .iter()
            .find(|i| i["label"].as_str() == Some("resolveMe"))
            .cloned()
            .expect("resolveMe must appear in completions for its own prefix");

        let resp = client.request("completionItem/resolve", resolve_me).await;

        assert!(
            resp["error"].is_null(),
            "completionItem/resolve error: {:?}",
            resp
        );
        assert!(resp["result"].is_object(), "expected resolved item object");
        // signature_for_symbol_from_index must populate `detail` with the function signature.
        let detail = resp["result"]["detail"].as_str().unwrap_or("");
        assert!(
            detail.contains("resolveMe"),
            "resolved item must have detail populated with the function signature, got: {:?}",
            resp["result"]
        );
    }

    // ── inlay hint resolve ────────────────────────────────────────────────────

    #[tokio::test]
    async fn inlay_hint_resolve_returns_hint() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ih_resolve.php",
            "<?php\nfunction add(int $a, int $b): int { return $a + $b; }\nadd(1, 2);\n",
        )
        .await;

        let hints_resp = client
            .request(
                "textDocument/inlayHint",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ih_resolve.php" },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 3, "character": 0 }
                    }
                }),
            )
            .await;

        let hints = hints_resp["result"].as_array().cloned().unwrap_or_default();
        assert!(
            !hints.is_empty(),
            "expected inlay hints for add(1, 2) call, got: {:?}",
            hints_resp["result"]
        );

        let resp = client.request("inlayHint/resolve", hints[0].clone()).await;

        assert!(
            resp["error"].is_null(),
            "inlayHint/resolve error: {:?}",
            resp
        );
        assert!(resp["result"].is_object(), "expected resolved hint object");
    }

    // ── semantic tokens range ─────────────────────────────────────────────────

    #[tokio::test]
    async fn semantic_tokens_range_returns_data() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///st_range.php",
            "<?php\nfunction ranged(int $x): int { return $x; }\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/semanticTokens/range",
                serde_json::json!({
                    "textDocument": { "uri": "file:///st_range.php" },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 2, "character": 0 }
                    }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "semanticTokens/range error: {:?}",
            resp
        );
        let result = &resp["result"];
        assert!(!result.is_null(), "expected non-null result");
        let data = result["data"]
            .as_array()
            .expect("expected data array in result");
        assert!(
            !data.is_empty(),
            "expected non-empty token data for a file with typed function"
        );
    }

    // ── semantic tokens full delta ────────────────────────────────────────────

    #[tokio::test]
    async fn semantic_tokens_full_delta_returns_result() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///st_delta.php",
            "<?php\nfunction delta(int $x): int { return $x; }\n",
        )
        .await;

        let full = client
            .request(
                "textDocument/semanticTokens/full",
                serde_json::json!({
                    "textDocument": { "uri": "file:///st_delta.php" }
                }),
            )
            .await;

        assert!(
            full["error"].is_null(),
            "semanticTokens/full error: {:?}",
            full
        );
        let result_id = full["result"]["resultId"].clone();
        assert!(
            !result_id.is_null(),
            "semanticTokens/full must return a resultId to support delta requests"
        );

        let resp = client
            .request(
                "textDocument/semanticTokens/full/delta",
                serde_json::json!({
                    "textDocument": { "uri": "file:///st_delta.php" },
                    "previousResultId": result_id
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "semanticTokens/full/delta error: {:?}",
            resp
        );
        let result = &resp["result"];
        assert!(
            result["edits"].is_array() || result["data"].is_array(),
            "expected 'edits' or 'data' in delta result, got: {:?}",
            result
        );
    }

    // ── document link ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn document_link_returns_array() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///dlink.php",
            "<?php\nrequire_once 'vendor/autoload.php';\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/documentLink",
                serde_json::json!({
                    "textDocument": { "uri": "file:///dlink.php" }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "documentLink error: {:?}", resp);
        // require_once with a string path must always produce at least one link entry.
        let links = resp["result"]
            .as_array()
            .expect("documentLink must return an array");
        assert!(
            !links.is_empty(),
            "expected at least one link for require_once path"
        );
    }

    // ── inline value ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn inline_value_returns_array() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///inlval.php",
            "<?php\n$x = 42;\n$y = $x + 1;\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/inlineValue",
                serde_json::json!({
                    "textDocument": { "uri": "file:///inlval.php" },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 3, "character": 0 }
                    },
                    "context": {
                        "frameId": 0,
                        "stoppedLocation": {
                            "start": { "line": 2, "character": 0 },
                            "end": { "line": 2, "character": 10 }
                        }
                    }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "inlineValue error: {:?}", resp);
        // $x and $y are in range so the server must return a non-null array.
        let values = resp["result"]
            .as_array()
            .expect("inlineValue must return an array when variables are in range");
        assert!(
            !values.is_empty(),
            "expected at least one inline value for $x/$y"
        );
    }

    // ── pull diagnostics ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn pull_diagnostics_returns_report() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(&mut client, "file:///pull_diag.php", "<?php\n$x = 1;\n").await;

        let resp = client
            .request(
                "textDocument/diagnostic",
                serde_json::json!({
                    "textDocument": { "uri": "file:///pull_diag.php" }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "textDocument/diagnostic error: {:?}",
            resp
        );
        let result = &resp["result"];
        assert!(!result.is_null(), "expected non-null diagnostic report");
        let kind = result["kind"].as_str().unwrap_or("");
        assert!(
            kind == "full" || kind == "unchanged",
            "expected kind 'full' or 'unchanged', got: {:?}",
            kind
        );
    }

    // ── workspace diagnostic ──────────────────────────────────────────────────

    #[tokio::test]
    async fn workspace_diagnostic_returns_report() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(&mut client, "file:///ws_diag.php", "<?php\n$x = 1;\n").await;

        let resp = client
            .request(
                "workspace/diagnostic",
                serde_json::json!({ "previousResultIds": [] }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "workspace/diagnostic error: {:?}",
            resp
        );
        let result = &resp["result"];
        let items = result["items"]
            .as_array()
            .expect("expected 'items' array in workspace diagnostic report");
        assert!(
            !items.is_empty(),
            "expected at least one item for the opened file, got empty items"
        );
    }

    // ── moniker ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn moniker_returns_no_error() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///moniker.php",
            "<?php\nfunction monikerFn(): void {}\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/moniker",
                serde_json::json!({
                    "textDocument": { "uri": "file:///moniker.php" },
                    "position": { "line": 1, "character": 9 }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "moniker error: {:?}", resp);
        let result = &resp["result"];
        assert!(
            result.is_array() && !result.as_array().unwrap().is_empty(),
            "expected non-empty moniker array, got: {:?}",
            result
        );
        assert_eq!(
            result[0]["identifier"].as_str().unwrap_or(""),
            "monikerFn",
            "expected moniker identifier 'monikerFn', got: {:?}",
            result[0]
        );
        assert_eq!(
            result[0]["scheme"].as_str().unwrap_or(""),
            "php",
            "expected moniker scheme 'php'"
        );
    }

    // ── linked editing range ──────────────────────────────────────────────────

    #[tokio::test]
    async fn linked_editing_range_returns_no_error() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///linked.php",
            "<?php\nclass LinkedClass {}\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/linkedEditingRange",
                serde_json::json!({
                    "textDocument": { "uri": "file:///linked.php" },
                    "position": { "line": 1, "character": 6 }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "linkedEditingRange error: {:?}",
            resp
        );
        let result = &resp["result"];
        assert!(
            !result.is_null(),
            "expected non-null LinkedEditingRanges for class name, got null"
        );
        let ranges = result["ranges"]
            .as_array()
            .expect("expected 'ranges' array in LinkedEditingRanges");
        assert!(
            !ranges.is_empty(),
            "expected at least one range for LinkedClass"
        );
    }

    // ── formatting ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn formatting_returns_null_or_edits() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///fmt.php",
            "<?php\nfunction ugly( $x ){return $x;}\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/formatting",
                serde_json::json!({
                    "textDocument": { "uri": "file:///fmt.php" },
                    "options": { "tabSize": 4, "insertSpaces": true }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "formatting error: {:?}", resp);
        assert!(
            resp["result"].is_null() || resp["result"].is_array(),
            "expected null or array, got: {:?}",
            resp["result"]
        );
    }

    #[tokio::test]
    async fn range_formatting_returns_null_or_edits() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///rfmt.php",
            "<?php\nfunction ugly( $x ){return $x;}\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/rangeFormatting",
                serde_json::json!({
                    "textDocument": { "uri": "file:///rfmt.php" },
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 2, "character": 0 }
                    },
                    "options": { "tabSize": 4, "insertSpaces": true }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "rangeFormatting error: {:?}", resp);
        assert!(
            resp["result"].is_null() || resp["result"].is_array(),
            "expected null or array, got: {:?}",
            resp["result"]
        );
    }

    // ── on-type formatting ────────────────────────────────────────────────────

    #[tokio::test]
    async fn on_type_formatting_returns_null_or_edits() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(&mut client, "file:///otfmt.php", "<?php\nif (true) {\n").await;

        let resp = client
            .request(
                "textDocument/onTypeFormatting",
                serde_json::json!({
                    "textDocument": { "uri": "file:///otfmt.php" },
                    "position": { "line": 1, "character": 10 },
                    "ch": "{",
                    "options": { "tabSize": 4, "insertSpaces": true }
                }),
            )
            .await;

        assert!(
            resp["error"].is_null(),
            "onTypeFormatting error: {:?}",
            resp
        );
        assert!(
            resp["result"].is_null() || resp["result"].is_array(),
            "expected null or array, got: {:?}",
            resp["result"]
        );
    }

    // ── code actions ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn code_action_phpdoc_offered_for_undocumented_function() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ca_phpdoc.php",
            "<?php\nfunction noDoc(int $x): int { return $x; }\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ca_phpdoc.php" },
                    "range": {
                        "start": { "line": 1, "character": 9 },
                        "end": { "line": 1, "character": 14 }
                    },
                    "context": { "diagnostics": [] }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
        let actions = resp["result"].as_array().cloned().unwrap_or_default();
        let has_phpdoc = actions.iter().any(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("phpdoc"))
                .unwrap_or(false)
        });
        assert!(has_phpdoc, "expected a PHPDoc action, got: {:?}", actions);
    }

    #[tokio::test]
    async fn code_action_extract_variable_offered_on_expression() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ca_extract.php",
            "<?php\n$result = 1 + 2;\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ca_extract.php" },
                    "range": {
                        "start": { "line": 1, "character": 10 },
                        "end": { "line": 1, "character": 15 }
                    },
                    "context": { "diagnostics": [] }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
        let actions = resp["result"].as_array().cloned().unwrap_or_default();
        let has_extract = actions.iter().any(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("extract"))
                .unwrap_or(false)
        });
        assert!(
            has_extract,
            "expected an Extract action, got: {:?}",
            actions
        );
    }

    #[tokio::test]
    async fn code_action_generate_constructor_offered_for_class() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ca_ctor.php",
            "<?php\nclass Point {\n    public int $x;\n    public int $y;\n}\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ca_ctor.php" },
                    "range": {
                        "start": { "line": 1, "character": 6 },
                        "end": { "line": 1, "character": 11 }
                    },
                    "context": { "diagnostics": [] }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
        let actions = resp["result"].as_array().cloned().unwrap_or_default();
        let has_ctor = actions.iter().any(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("constructor"))
                .unwrap_or(false)
        });
        assert!(
            has_ctor,
            "expected a Generate constructor action, got: {:?}",
            actions
        );
    }

    #[tokio::test]
    async fn code_action_implement_missing_offered() {
        let mut client = start_server();
        initialize(&mut client).await;

        // Interface and class in the same file — previously broken, now fixed.
        open_doc(
            &mut client,
            "file:///ca_impl.php",
            "<?php\ninterface Greetable {\n    public function greet(): string;\n}\nclass Hello implements Greetable {\n}\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ca_impl.php" },
                    "range": {
                        "start": { "line": 4, "character": 0 },
                        "end": { "line": 4, "character": 0 }
                    },
                    "context": { "diagnostics": [] }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
        let actions = resp["result"].as_array().cloned().unwrap_or_default();
        let has_impl = actions.iter().any(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("implement"))
                .unwrap_or(false)
        });
        assert!(has_impl, "expected an Implement action, got: {:?}", actions);
    }

    #[tokio::test]
    async fn code_action_add_return_type_offered() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///ca_rettype.php",
            "<?php\nfunction noReturn() { return 42; }\n",
        )
        .await;

        let resp = client
            .request(
                "textDocument/codeAction",
                serde_json::json!({
                    "textDocument": { "uri": "file:///ca_rettype.php" },
                    "range": {
                        "start": { "line": 1, "character": 9 },
                        "end": { "line": 1, "character": 17 }
                    },
                    "context": { "diagnostics": [] }
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "codeAction error: {:?}", resp);
        let actions = resp["result"].as_array().cloned().unwrap_or_default();
        let has_ret = actions.iter().any(|a| {
            a["title"]
                .as_str()
                .map(|t| t.to_lowercase().contains("return type"))
                .unwrap_or(false)
        });
        assert!(
            has_ret,
            "expected an Add return type action, got: {:?}",
            actions
        );
    }

    // ── file lifecycle notifications ──────────────────────────────────────────

    #[tokio::test]
    async fn will_rename_files_returns_no_error() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///rename_old.php",
            "<?php\nclass OldClass {}\n",
        )
        .await;

        let resp = client
            .request(
                "workspace/willRenameFiles",
                serde_json::json!({
                    "files": [{
                        "oldUri": "file:///rename_old.php",
                        "newUri": "file:///rename_new.php"
                    }]
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "willRenameFiles error: {:?}", resp);
        assert!(
            resp["result"].is_null() || resp["result"].is_object(),
            "expected null or WorkspaceEdit, got: {:?}",
            resp["result"]
        );
    }

    #[tokio::test]
    async fn will_create_files_returns_no_error() {
        let mut client = start_server();
        initialize(&mut client).await;

        let resp = client
            .request(
                "workspace/willCreateFiles",
                serde_json::json!({
                    "files": [{ "uri": "file:///new_created.php" }]
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "willCreateFiles error: {:?}", resp);
        assert!(
            resp["result"].is_null() || resp["result"].is_object(),
            "expected null or WorkspaceEdit, got: {:?}",
            resp["result"]
        );
    }

    #[tokio::test]
    async fn will_delete_files_returns_no_error() {
        let mut client = start_server();
        initialize(&mut client).await;

        open_doc(
            &mut client,
            "file:///to_delete.php",
            "<?php\nclass ToDelete {}\n",
        )
        .await;

        let resp = client
            .request(
                "workspace/willDeleteFiles",
                serde_json::json!({
                    "files": [{ "uri": "file:///to_delete.php" }]
                }),
            )
            .await;

        assert!(resp["error"].is_null(), "willDeleteFiles error: {:?}", resp);
        assert!(
            resp["result"].is_null() || resp["result"].is_object(),
            "expected null or WorkspaceEdit, got: {:?}",
            resp["result"]
        );
    }
}
