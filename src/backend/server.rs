#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::Arc;

use arc_swap::ArcSwap;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::notification::Progress as ProgressNotification;
use tower_lsp::lsp_types::request::WorkDoneProgressCreate;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, async_trait};

use php_ast::{
    ClassMember, ClassMemberKind, EnumMember, EnumMemberKind, ExprKind, NamespaceBody, Stmt,
    StmtKind,
};

use crate::ast::{ParsedDoc, str_offset};
use crate::autoload::Psr4Map;
use crate::completion::{CompletionCtx, filtered_completions_at};
use crate::config::LspConfig;
use crate::document_store::DocumentStore;
use crate::file_rename::{use_edits_for_delete, use_edits_for_rename};
use crate::hover::{
    class_hover_from_index, docs_for_symbol_from_index, hover_info_with_maps,
    signature_for_symbol_from_index,
};
use crate::open_files::{OpenFiles, compute_open_file_diagnostics};
use crate::panic_guard::{guard_async, guard_async_result};
use crate::phpstorm_meta::PhpStormMeta;
use crate::symbols::{
    document_symbols, resolve_workspace_symbol, workspace_symbols_from_workspace,
};
use crate::use_import::{build_use_import_edit, find_fqn_for_class};
use crate::util::{fqn_short_name, word_at_position};
use crate::workspace_scan::{scan_workspace, send_refresh_requests};

use crate::actions::extract_action::extract_variable_actions;
use crate::actions::extract_constant_action::extract_constant_actions;
use crate::actions::extract_method_action::extract_method_actions;
use crate::actions::generate_action::{
    generate_constructor_actions, generate_getters_setters_actions,
};
use crate::actions::implement_action::implement_missing_actions;
use crate::actions::inline_action::inline_variable_actions;
use crate::actions::phpdoc_action::phpdoc_actions;
use crate::actions::promote_action::promote_constructor_actions;
use crate::actions::type_action::add_return_type_actions;

use crate::navigation::call_hierarchy::{incoming_calls, outgoing_calls, prepare_call_hierarchy};
use crate::navigation::declaration::{goto_declaration, goto_declaration_from_index};
use crate::navigation::definition::{
    find_declaration_in_indexes, find_declaration_range, find_method_in_class_hierarchy,
    goto_definition,
};
use crate::navigation::implementation::{
    find_implementations, find_implementations_from_workspace,
};
use crate::navigation::moniker::moniker_at;
use crate::navigation::references::{
    SymbolKind, find_constructor_references, find_references, find_references_with_target,
};
use crate::navigation::type_definition::{goto_type_definition, goto_type_definition_from_index};
use crate::navigation::type_hierarchy::{
    prepare_type_hierarchy_from_workspace, subtypes_of_from_workspace, supertypes_of_from_workspace,
};

use crate::analysis::code_lens::code_lenses;
use crate::analysis::diagnostics::{
    merge_file_diagnostics, parse_document, parse_document_no_diags,
};
use crate::analysis::document_highlight::document_highlights;
use crate::analysis::inlay_hints::inlay_hints;
use crate::analysis::inline_value::inline_values_in_range;
use crate::analysis::semantic_diagnostics::duplicate_declaration_diagnostics;
use crate::analysis::semantic_tokens::{
    compute_token_delta, legend, semantic_tokens, semantic_tokens_range, token_hash,
};

use crate::editing::document_link::document_links;
use crate::editing::folding::folding_ranges;
use crate::editing::formatting::{format_document, format_range};
use crate::editing::on_type_format::on_type_format;
use crate::editing::organize_imports::organize_imports_action;
use crate::editing::rename::{prepare_rename, rename, rename_property, rename_variable};
use crate::editing::selection_range::selection_ranges;
use crate::editing::signature_help::signature_help;

use super::helpers::{
    DEFERRED_ACTION_TAGS, class_name_at_construct_decl, cursor_is_on_constant_decl,
    cursor_is_on_method_decl, cursor_is_on_property_decl, defer_actions, is_after_arrow,
    php_file_op, position_to_byte_offset, promoted_property_at_cursor, range_within, run_phpunit,
    symbol_kind_at,
};
use super::{
    Backend, IndexReadyNotification, build_mir_symbol, compute_dependent_publishes_owned,
    compute_diagnostic_result_id, resolve_reference_symbol,
};

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
            self.root_paths.store(Arc::new(roots));
        }

        {
            let opts = params.initialization_options.as_ref();
            let roots = self.root_paths.load_full();
            let file_cfg = crate::autoload::load_project_config_json(&roots);

            if matches!(file_cfg, Some(serde_json::Value::Null)) {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        "php-lsp: .php-lsp.json contains invalid JSON — ignoring",
                    )
                    .await;
            }

            if let Some(serde_json::Value::Object(ref obj)) = file_cfg
                && let Some(ver) = obj.get("phpVersion").and_then(|v| v.as_str())
                && !crate::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: .php-lsp.json unsupported phpVersion {ver:?} — valid values: {}",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }

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

            // Merge: file config is the base; editor initializationOptions override per-key.
            // excludePaths arrays are concatenated rather than replaced.
            let file_obj = file_cfg.as_ref().filter(|v| v.is_object());
            let merged = LspConfig::merge_project_configs(file_obj, opts);
            let mut cfg = LspConfig::from_value(&merged);

            // PSR-4 loading and PHP version resolution both involve blocking I/O
            // (filesystem reads, and potentially spawning `php --version`). Run
            // them concurrently on the blocking thread pool so initialize responds
            // in max(psr4_time, version_time) rather than psr4_time + version_time.
            let roots_for_psr4 = (*roots).clone();
            let roots_for_ver = (*roots).clone();
            let explicit_version = cfg.php_version.clone();
            let (psr4_result, ver_result) = tokio::join!(
                tokio::task::spawn_blocking(move || {
                    let mut merged = Psr4Map::empty();
                    for root in &roots_for_psr4 {
                        merged.extend(Psr4Map::load(root));
                    }
                    merged
                }),
                tokio::task::spawn_blocking(move || {
                    crate::autoload::resolve_php_version_from_roots(
                        &roots_for_ver,
                        explicit_version.as_deref(),
                    )
                }),
            );
            if let Ok(psr4) = psr4_result {
                self.psr4.store(Arc::new(psr4));
            }
            let (ver, source) =
                ver_result.unwrap_or_else(|_| (crate::autoload::PHP_8_5.to_string(), "default"));
            self.client
                .log_message(
                    tower_lsp::lsp_types::MessageType::INFO,
                    format!("php-lsp: using PHP {ver} ({source})"),
                )
                .await;
            let ver = if source != "set by editor" && !crate::autoload::is_valid_php_version(&ver) {
                let clamped = crate::autoload::clamp_php_version(&ver);
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range \
                                 ({}); using PHP {clamped} for analysis",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
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
        }

        let feat = self.config.load().features.clone();
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
                completion_provider: feat.completion.then(|| CompletionOptions {
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
                hover_provider: feat.hover.then_some(HoverProviderCapability::Simple(true)),
                definition_provider: feat.definition.then_some(OneOf::Left(true)),
                references_provider: feat.references.then_some(OneOf::Left(true)),
                document_symbol_provider: feat.document_symbols.then_some(OneOf::Left(true)),
                workspace_symbol_provider: feat.workspace_symbols.then(|| {
                    OneOf::Right(WorkspaceSymbolOptions {
                        resolve_provider: Some(true),
                        work_done_progress_options: Default::default(),
                    })
                }),
                rename_provider: feat.rename.then(|| {
                    OneOf::Right(RenameOptions {
                        prepare_provider: Some(true),
                        work_done_progress_options: Default::default(),
                    })
                }),
                signature_help_provider: feat.signature_help.then(|| SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                inlay_hint_provider: feat.inlay_hints.then(|| {
                    OneOf::Right(InlayHintServerCapabilities::Options(InlayHintOptions {
                        resolve_provider: Some(true),
                        work_done_progress_options: Default::default(),
                    }))
                }),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                semantic_tokens_provider: feat.semantic_tokens.then(|| {
                    SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                        legend: legend(),
                        full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
                        range: Some(true),
                        ..Default::default()
                    })
                }),
                selection_range_provider: feat
                    .selection_range
                    .then_some(SelectionRangeProviderCapability::Simple(true)),
                call_hierarchy_provider: feat
                    .call_hierarchy
                    .then_some(CallHierarchyServerCapability::Simple(true)),
                document_highlight_provider: feat.document_highlight.then_some(OneOf::Left(true)),
                implementation_provider: feat
                    .implementation
                    .then_some(ImplementationProviderCapability::Simple(true)),
                code_action_provider: feat.code_action.then(|| {
                    CodeActionProviderCapability::Options(CodeActionOptions {
                        resolve_provider: Some(true),
                        ..Default::default()
                    })
                }),
                declaration_provider: feat
                    .declaration
                    .then_some(DeclarationCapability::Simple(true)),
                type_definition_provider: feat
                    .type_definition
                    .then_some(TypeDefinitionProviderCapability::Simple(true)),
                code_lens_provider: feat.code_lens.then_some(CodeLensOptions {
                    resolve_provider: Some(true),
                }),
                document_formatting_provider: feat.formatting.then_some(OneOf::Left(true)),
                document_range_formatting_provider: feat
                    .range_formatting
                    .then_some(OneOf::Left(true)),
                document_on_type_formatting_provider: feat.on_type_formatting.then(|| {
                    DocumentOnTypeFormattingOptions {
                        first_trigger_character: "}".to_string(),
                        more_trigger_character: Some(vec!["\n".to_string()]),
                    }
                }),
                document_link_provider: feat.document_link.then(|| DocumentLinkOptions {
                    resolve_provider: Some(true),
                    work_done_progress_options: Default::default(),
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["php-lsp.runTest".to_string()],
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
                linked_editing_range_provider: feat
                    .linked_editing_range
                    .then_some(LinkedEditingRangeServerCapabilities::Simple(true)),
                moniker_provider: Some(OneOf::Left(true)),
                inline_value_provider: feat.inline_values.then(|| {
                    OneOf::Right(InlineValueServerCapabilities::Options(InlineValueOptions {
                        work_done_progress_options: Default::default(),
                    }))
                }),
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
            Registration {
                id: "php-lsp-config-change".to_string(),
                method: "workspace/didChangeConfiguration".to_string(),
                register_options: Some(serde_json::json!({"section": "php-lsp"})),
            },
        ];
        self.client.register_capability(registrations).await.ok();

        let roots: Vec<PathBuf> = (**self.root_paths.load()).clone();
        if !roots.is_empty() {
            {
                let mut merged = Psr4Map::empty();
                for root in &roots {
                    merged.extend(Psr4Map::load(root));
                }
                self.psr4.store(Arc::new(merged));
            }
            self.meta.store(Arc::new(PhpStormMeta::load(&roots[0])));

            let token = NumberOrString::String("php-lsp/indexing".to_string());
            self.client
                .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                    token: token.clone(),
                })
                .await
                .ok();

            // Background-warm the mir AnalysisSession so the first did_open
            // doesn't pay ~80ms of stub loading.  Fired before the scan so
            // warm-up and file I/O overlap on separate blocking threads.
            let warm_docs = Arc::clone(&self.docs);
            tokio::task::spawn_blocking(move || {
                let php_version = warm_docs.workspace_php_version();
                warm_docs.analysis_session(php_version);
            });

            let docs = Arc::clone(&self.docs);
            let open_files = self.open_files.clone();
            let client = self.client.clone();
            let (exclude_paths, include_paths, max_indexed_files) = {
                let cfg = self.config.load();
                let mut exclude = cfg.exclude_paths.clone();
                // Lazy vendor: by default we skip `vendor/` during the eager
                // scan and rely on PSR-4 resolution (`psr4_goto`) to index
                // vendor files on demand. Users with workspaces small enough
                // to want full-vendor symbol search can opt in via
                // `indexVendor: true`.
                if !cfg.index_vendor && !exclude.iter().any(|p| p == "vendor" || p == "vendor/") {
                    exclude.push("vendor/".to_string());
                }
                (exclude, cfg.include_paths.clone(), cfg.max_indexed_files)
            };
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
                let mut session_cache_set = false;
                for root in roots {
                    // Phase K2b: open the on-disk cache for this root. If the
                    // system has no usable cache dir (weird XDG env, sandboxed
                    // runner, read-only home), `new` returns None and every
                    // per-file `cache.as_ref()` guard below no-ops — scan still
                    // runs, just without persistence.
                    let cache = crate::cache::WorkspaceCache::new(&root);
                    // Wire the first available cache directory into the
                    // AnalysisSession builder so stub-parse results survive
                    // server restarts.
                    if !session_cache_set && let Some(ref c) = cache {
                        let session_dir = c.cache_dir().join("session");
                        docs.set_session_cache_dir(session_dir);
                        session_cache_set = true;
                    }
                    total += scan_workspace(
                        root,
                        Arc::clone(&docs),
                        open_files.clone(),
                        cache,
                        &exclude_paths,
                        &include_paths,
                        max_indexed_files,
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

                // Phase D: reference index is lazy. `textDocument/references`
                // drives `symbol_refs(ws, key)` on demand; salsa memoizes the
                // per-file `file_refs` across requests. Invalidation is
                // automatic on edits.
                //
                // Phase L: warm the memo in the background so the first real
                // reference lookup doesn't pay the full-workspace walk.
                // `symbol_refs(ws, <any key>)` iterates every file's
                // `file_refs` to build its result — even with a sentinel key
                // that matches nothing, the per-file walk runs and populates
                // salsa's memo. Fire-and-forget: a reference request that
                // arrives mid-warmup just retries through
                // `snapshot_query`'s `salsa::Cancelled` handling.
                let warm_docs = Arc::clone(&docs);
                tokio::task::spawn_blocking(move || {
                    // Pre-compute file_index for every workspace file so the first
                    // hover/completion does not pay the full parse cost at request time.
                    warm_docs.get_workspace_index_salsa();
                })
                .await
                .ok();
                drop(docs);
                client.send_notification::<IndexReadyNotification>(()).await;
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
            let roots = self.root_paths.load_full();

            // Re-read .php-lsp.json so a user who edits the file and then
            // triggers a configuration reload picks up the latest values.
            let file_cfg = crate::autoload::load_project_config_json(&roots);

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
            let ver = if source != "set by editor" && !crate::autoload::is_valid_php_version(&ver) {
                let clamped = crate::autoload::clamp_php_version(&ver);
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range ({}); \
                             using PHP {clamped} for analysis",
                            crate::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
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
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
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
        let (exclude_paths, include_paths, max_indexed_files) = {
            let cfg = self.config.load();
            (
                cfg.exclude_paths.clone(),
                cfg.include_paths.clone(),
                cfg.max_indexed_files,
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
                    tokio::spawn(async move {
                        let cache = crate::cache::WorkspaceCache::new(&path_clone);
                        scan_workspace(
                            path_clone,
                            docs,
                            open_files,
                            cache,
                            &ex,
                            &ip,
                            max_indexed_files,
                        )
                        .await;
                        send_refresh_requests(&client).await;
                    });
                }
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
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
            self.set_open_text(uri.clone(), text.clone());

            let docs_for_spawn = Arc::clone(&self.docs);
            let diag_cfg = self.config.load().diagnostics.clone();

            // Phase I: semantic analysis on the blocking pool (CPU-bound, hundreds
            // of ms on cold files).  We skip the redundant standalone parse_document
            // call that used to precede this — get_semantic_issues_salsa already
            // parses via salsa internally and caches the ParsedDoc.  Parse errors
            // are read from that cached doc after the blocking task completes.
            let uri_sem = uri.clone();
            let sem_issues = tokio::task::spawn_blocking(move || {
                docs_for_spawn.get_semantic_issues_salsa(&uri_sem)
            })
            .await
            .unwrap_or(None);

            // Extract parse errors from the salsa-cached ParsedDoc.  On the fast
            // path this is a lock-free DashMap lookup — no re-parse.
            let parse_diags = self
                .docs
                .get_doc_salsa(&uri)
                .map(|doc| crate::analysis::diagnostics::diagnostics_from_doc(&doc))
                .unwrap_or_default();

            self.set_parse_diagnostics(&uri, parse_diags.clone());
            let stored_source = self.get_open_text(&uri).unwrap_or_default();
            let doc2 = self.get_doc(&uri);
            let dup_decl = doc2
                .as_ref()
                .map(|d| duplicate_declaration_diagnostics(&stored_source, d, &diag_cfg))
                .unwrap_or_default();
            let semantic = sem_issues
                .map(|issues| {
                    crate::semantic_diagnostics::issues_to_diagnostics(&issues, &uri, &diag_cfg)
                })
                .unwrap_or_default();
            let all_diags = merge_file_diagnostics(parse_diags, dup_decl, semantic);
            // Publish for the opened file FIRST — see did_change for why ordering matters.
            self.client
                .publish_diagnostics(uri.clone(), all_diags, None)
                .await;

            // Cross-file republish via the session's parallel re-analysis API.
            // Only files whose Pass-2 actually changed appear in the result —
            // we don't blast every open file with a publish like the old loop.
            let dependents = self.compute_dependent_publishes(&uri, &diag_cfg).await;
            for (dep_uri, dep_diags) in dependents {
                self.client
                    .publish_diagnostics(dep_uri, dep_diags, None)
                    .await;
            }
        })
        .await
    }

    #[tracing::instrument(skip_all)]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        guard_async("did_change", async move {
            let uri = params.text_document.uri;
            let text = match params.content_changes.into_iter().last() {
                Some(c) => c.text,
                None => return,
            };

            // Store text immediately and capture the version token.
            // Features (completion, hover, …) see the new text instantly while
            // the parse runs in the background.
            let version = self.set_open_text(uri.clone(), text.clone());

            let docs = Arc::clone(&self.docs);
            let open_files = self.open_files.clone();
            let client = self.client.clone();
            let diag_cfg = self.config.load().diagnostics.clone();
            tokio::spawn(async move {
                // 100 ms debounce: if another edit arrives before we parse,
                // the version gate in Backend below will discard this result.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                let (_doc, diagnostics) =
                    tokio::task::spawn_blocking(move || parse_document(&text))
                        .await
                        .unwrap_or_else(|_| (ParsedDoc::default(), vec![]));

                // Only apply if no newer edit arrived while we were parsing.
                // Backend-level gate replaces the old `apply_parse` version check.
                if open_files.current_version(&uri) == Some(version) {
                    open_files.set_parse_diagnostics(&uri, diagnostics.clone());

                    // Phase I: the salsa `semantic_issues` walk is synchronous
                    // and CPU-bound on a cold file — run it on the blocking
                    // pool so the async runtime stays responsive. Returns the
                    // full diagnostic bundle (semantic + dup-decl + deprecated
                    // calls), all computed off-thread.
                    let docs_sem = Arc::clone(&docs);
                    let open_files_sem = open_files.clone();
                    let uri_sem = uri.clone();
                    let diag_cfg_sem = diag_cfg.clone();
                    let (extra_dup, extra_sem) = tokio::task::spawn_blocking(move || {
                        let Some(d) = open_files_sem.get_doc(&docs_sem, &uri_sem) else {
                            return (Vec::<Diagnostic>::new(), Vec::<Diagnostic>::new());
                        };
                        let source = open_files_sem.text(&uri_sem).unwrap_or_default();
                        let dup = duplicate_declaration_diagnostics(&source, &d, &diag_cfg_sem);
                        let sem = docs_sem
                            .get_semantic_issues_salsa(&uri_sem)
                            .map(|issues| {
                                crate::semantic_diagnostics::issues_to_diagnostics(
                                    &issues,
                                    &uri_sem,
                                    &diag_cfg_sem,
                                )
                            })
                            .unwrap_or_default();
                        (dup, sem)
                    })
                    .await
                    .unwrap_or_default();

                    let all_diags = merge_file_diagnostics(diagnostics, extra_dup, extra_sem);
                    // Publish for the changed file FIRST. Test harnesses (and
                    // some clients) consume publishDiagnostics for unrelated
                    // URIs while waiting for one specific URI; reversing this
                    // order would silently swallow the changed file's publish.
                    client
                        .publish_diagnostics(uri.clone(), all_diags, None)
                        .await;

                    // Cross-file republish via the session's parallel
                    // re-analysis API. Only files whose Pass-2 changed are
                    // returned — the old loop blasted every open file.
                    //
                    // Race window: if `other` is being edited concurrently,
                    // its own debounced did_change will still fire a republish,
                    // so any briefly-stale publish here self-corrects within
                    // ~100 ms.
                    let dependents = compute_dependent_publishes_owned(
                        Arc::clone(&docs),
                        open_files.clone(),
                        uri.clone(),
                        diag_cfg.clone(),
                    )
                    .await;
                    for (dep_uri, dep_diags) in dependents {
                        client.publish_diagnostics(dep_uri, dep_diags, None).await;
                    }
                }
            });
        })
        .await
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.close_open_file(&uri);
        // Clear editor diagnostics; the file stays indexed for cross-file features
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn will_save(&self, _params: WillSaveTextDocumentParams) {}

    async fn will_save_wait_until(
        &self,
        params: WillSaveTextDocumentParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let source = self
            .get_open_text(&params.text_document.uri)
            .unwrap_or_default();
        Ok(format_document(&source))
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        // Re-publish diagnostics on save so editors that defer diagnostics
        // until save (rather than on every keystroke) see up-to-date results.
        // Must include semantic diagnostics — publishDiagnostics replaces the
        // prior set entirely, so omitting them would clear errors the editor
        // showed after the last did_change.
        let diag_cfg = self.config.load().diagnostics.clone();
        let all = compute_open_file_diagnostics(&self.docs, &self.open_files, &uri, &diag_cfg);
        self.client.publish_diagnostics(uri, all, None).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            match change.typ {
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    if let Ok(path) = change.uri.to_file_path()
                        && let Ok(text) = tokio::fs::read_to_string(&path).await
                    {
                        // Salsa path: index_from_doc mirrors the new text into
                        // the SourceFile input. On the next codebase() call,
                        // salsa re-runs file_definitions for this file and the
                        // aggregator re-folds — no manual remove/collect/finalize.
                        let doc = parse_document_no_diags(&text);
                        self.index_from_doc_if_not_open(change.uri.clone(), &doc);
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
            let trigger = params
                .context
                .as_ref()
                .and_then(|c| c.trigger_character.as_deref());
            let meta_loaded = self.meta.load();
            let meta_opt = if meta_loaded.is_empty() {
                None
            } else {
                Some(&**meta_loaded)
            };
            let imports = self.file_imports(uri);
            let wi = self.docs.get_workspace_index_salsa();
            let docs_for_lookup = Arc::clone(&self.docs);
            let find_class_doc_fn = move |name: &str| -> Option<Arc<ParsedDoc>> {
                let cr = *wi.classes_by_name.get(name)?.first()?;
                let (uri, _) = wi.at(cr)?;
                docs_for_lookup.get_doc_salsa(uri)
            };
            let analysis = self.docs.cached_analysis(uri);
            let ctx = CompletionCtx {
                source: Some(&source),
                position: Some(position),
                meta: meta_opt,
                doc_uri: Some(uri),
                file_imports: Some(&imports),
                find_class_doc: Some(&find_class_doc_fn),
                analysis: analysis.as_deref(),
            };
            Ok(Some(CompletionResponse::Array(filtered_completions_at(
                &doc,
                &other_docs,
                trigger,
                &ctx,
            ))))
        })
        .await
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
        guard_async_result("goto_definition", async move {
            let uri = &params.text_document_position_params.text_document.uri;
            let position = params.text_document_position_params.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let doc = match self.get_doc(uri) {
                Some(d) => d,
                None => return Ok(None),
            };
            // Search current file's ParsedDoc first (fast), then fall back to index search.
            if let Some(loc) = goto_definition(uri, &source, &doc, &[], position) {
                return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
            }
            // Receiver-aware method dispatch: `$var->method()` must jump to the
            // method defined in `$var`'s class hierarchy, not the first `method`
            // found in any indexed file (which would return a wrong class).
            if let Some(line_text) = source.lines().nth(position.line as usize)
                && let Some(word) = crate::util::word_at_position(&source, position)
                && let Some(receiver) = crate::hover::extract_receiver_var_before_cursor(
                    line_text,
                    position.character as usize,
                )
            {
                let class_name = if receiver == "$this" {
                    crate::type_map::enclosing_class_at(&source, &doc, position)
                } else {
                    let tm = crate::type_map::TypeMap::from_doc_at_position(&doc, None, position);
                    tm.get(&receiver).map(|s| s.to_string())
                };
                if let Some(cls) = class_name {
                    let first_cls = cls.split('|').next().unwrap_or(&cls).to_owned();
                    let all_indexes = self.docs.all_indexes();
                    if let Some(loc) =
                        find_method_in_class_hierarchy(&first_cls, &word, &all_indexes)
                    {
                        let refined = self
                            .docs
                            .get_doc_salsa(&loc.uri)
                            .and_then(|doc| {
                                find_declaration_range(doc.source(), &doc, &word).map(|range| {
                                    Location {
                                        uri: loc.uri.clone(),
                                        range,
                                    }
                                })
                            })
                            .unwrap_or(loc);
                        return Ok(Some(GotoDefinitionResponse::Scalar(refined)));
                    }
                }
            }

            // Cross-file: use FileIndex (no disk I/O for background files).
            let other_indexes = self.docs.other_indexes(uri);
            if let Some(word) = crate::util::word_at_position(&source, position)
                && let Some(loc) = find_declaration_in_indexes(&word, &other_indexes)
            {
                let refined = self
                    .docs
                    .get_doc_salsa(&loc.uri)
                    .and_then(|doc| {
                        find_declaration_range(doc.source(), &doc, &word).map(|range| Location {
                            uri: loc.uri.clone(),
                            range,
                        })
                    })
                    .unwrap_or(loc);
                return Ok(Some(GotoDefinitionResponse::Scalar(refined)));
            }

            // PSR-4 fallback: only useful for fully-qualified names (contain `\`)
            if let Some(word) = word_at_position(&source, position)
                && word.contains('\\')
                && let Some(loc) = self.psr4_goto(&word).await
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
            }

            Ok(None)
        })
        .await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        guard_async_result("references", async move {
            let uri = &params.text_document_position.text_document.uri;
            let position = params.text_document_position.position;
            let source = self.get_open_text(uri).unwrap_or_default();
            let word = match word_at_position(&source, position) {
                Some(w) => w,
                None => return Ok(None),
            };
            let include_declaration = params.context.include_declaration;

            // Special case: cursor on a class's `__construct` declaration — its
            // call sites are `new OwningClass(...)`, so name-only matching would
            // otherwise return every class's constructor declaration.
            if word == "__construct"
                && let Some(doc) = self.get_doc(uri)
                && let Some(class_name) =
                    class_name_at_construct_decl(doc.source(), &doc.program().stmts, position)
            {
                let locations = self.construct_references(
                    uri,
                    &source,
                    position,
                    &class_name,
                    include_declaration,
                );
                return Ok((!locations.is_empty()).then_some(locations));
            }

            let doc_opt = self.get_doc(uri);
            let (word, kind, constant_owner) =
                resolve_reference_symbol(doc_opt.as_ref(), &source, position, word);
            let all_docs = self.docs.all_docs_for_scan();
            let target_fqn = self.resolve_reference_target_fqn(
                uri,
                doc_opt.as_ref(),
                &word,
                kind,
                position,
                constant_owner,
            );

            // Ensure all workspace files are ingested before querying the
            // session — it only sees files that have been opened, so
            // background-indexed files would otherwise be invisible to
            // `references_to`.
            if matches!(kind, Some(SymbolKind::Method)) {
                self.docs.ensure_all_files_ingested();
            }
            let owner_short: Option<String> = if matches!(kind, Some(SymbolKind::Method)) {
                target_fqn
                    .as_deref()
                    .map(|fqn| fqn_short_name(fqn.trim_start_matches('\\')).to_string())
            } else {
                None
            };

            let session_method_refs = self.session_method_references(
                &word,
                kind,
                target_fqn.as_deref(),
                owner_short.as_deref(),
            );

            let mut locations = if let Some(session_locs) =
                session_method_refs.filter(|l| !l.is_empty())
            {
                // Session results are the authoritative call-site source for
                // methods. Push the cursor's own method-name span as the decl so
                // `include_declaration=true` still surfaces it — derived from the
                // identifier under the cursor (not the raw cursor position) so a
                // mid-identifier cursor doesn't shift the span right.
                let mut combined = session_locs;
                if include_declaration {
                    let range =
                        crate::util::word_range_at(&source, position).unwrap_or_else(|| Range {
                            start: position,
                            end: Position {
                                line: position.line,
                                character: position.character + word.len() as u32,
                            },
                        });
                    combined.push(Location {
                        uri: uri.clone(),
                        range,
                    });
                    crate::references::dedup_ref_locations(&mut combined);
                }
                combined
            } else {
                match target_fqn.as_deref() {
                    Some(t) => {
                        find_references_with_target(&word, &all_docs, include_declaration, kind, t)
                    }
                    None => find_references(&word, &all_docs, include_declaration, kind),
                }
            };

            // For Class / Function kinds: AST walker is authoritative; augment
            // with session refs to catch type-resolved sites the walker misses.
            if !matches!(kind, Some(SymbolKind::Method))
                && let Some(sym) = build_mir_symbol(&word, kind, target_fqn.as_deref())
            {
                let extra = self.docs.session_references_to(&sym);
                if !extra.is_empty() {
                    let mut seen: std::collections::HashSet<(String, u32, u32, u32)> = locations
                        .iter()
                        .map(crate::references::ref_location_key)
                        .collect();
                    for loc in extra
                        .into_iter()
                        .filter_map(crate::references::session_tuple_to_location)
                    {
                        if seen.insert(crate::references::ref_location_key(&loc)) {
                            locations.push(loc);
                        }
                    }
                }
            }

            Ok((!locations.is_empty()).then_some(locations))
        })
        .await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        Ok(prepare_rename(&source, params.position).map(PrepareRenameResponse::Range))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
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
            Ok(Some(rename_variable(
                &word,
                &params.new_name,
                uri,
                &doc,
                position,
            )))
        } else if is_after_arrow(&source, position) {
            let all_docs = self.docs.all_docs_for_scan();
            Ok(Some(rename_property(&word, &params.new_name, &all_docs)))
        } else {
            let all_docs = self.docs.all_docs_for_scan();
            let doc_opt = self.get_doc(uri);
            let target_fqn: Option<String> = doc_opt.as_ref().map(|doc| {
                let imports = self.file_imports(uri);
                crate::navigation::moniker::resolve_fqn(doc, &word, &imports)
            });
            Ok(Some(rename(
                &word,
                &params.new_name,
                &all_docs,
                target_fqn.as_deref(),
            )))
        }
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let all_indexes = self.docs.all_indexes();
        Ok(signature_help(&source, &doc, position, &all_indexes))
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
            let analysis = self.docs.cached_analysis(uri);
            let result = hover_info_with_maps(
                &source,
                &doc,
                analysis.as_deref(),
                position,
                &other_docs,
                &other_maps,
            );
            if result.is_some() {
                return Ok(result);
            }
            // Fallback: look up the word in the workspace index so class names in
            // extends clauses and parameter types resolve even when their defining
            // file is never opened.  Also try the alias-resolved name so that
            // `use Foo as Bar` works even when Foo is only in the index.
            if let Some(word) = crate::util::word_at_position(&source, position) {
                let wi = self.docs.get_workspace_index_salsa();
                // Try the literal word first.
                if let Some(h) = class_hover_from_index(&word, &wi.files) {
                    return Ok(Some(h));
                }
                // Try alias resolution.
                if let Some(resolved) = crate::hover::resolve_use_alias(&doc.program().stmts, &word)
                    && let Some(h) = class_hover_from_index(&resolved, &wi.files)
                {
                    return Ok(Some(h));
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
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
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
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let analysis = self.docs.cached_analysis(uri);
        let wi = self.docs.get_workspace_index_salsa();
        Ok(Some(inlay_hints(
            doc.source(),
            &doc,
            analysis.as_deref(),
            params.range,
            &wi.files,
        )))
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
        // Phase J: read through the salsa-memoized aggregate so repeated
        // workspace-symbol queries (every keystroke in the picker) share the
        // same `Arc` until a file changes.
        let wi = self.docs.get_workspace_index_salsa();
        let results = workspace_symbols_from_workspace(&params.query, &wi);
        Ok(Some(results))
    }

    async fn symbol_resolve(&self, params: WorkspaceSymbol) -> Result<WorkspaceSymbol> {
        // For resolve, we need the full range from the ParsedDoc of open files.
        let docs = self.docs.docs_for(&self.open_urls());
        Ok(resolve_workspace_symbol(params, &docs))
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
    }

    async fn semantic_tokens_full_delta(
        &self,
        params: SemanticTokensDeltaParams,
    ) -> Result<Option<SemanticTokensFullDeltaResult>> {
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
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
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
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let word = match word_at_position(&source, position) {
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
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
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
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        // Need the word at the cursor to know if this is a variable rename
        // (`$foo`) — the wordPattern we send back must require/forbid `$`
        // accordingly so that linked-mode typing produces valid PHP.
        let word = match crate::util::word_at_position(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        let is_variable = word.starts_with('$');
        let cursor_word_range = match crate::util::word_range_at(&source, position) {
            Some(r) => r,
            None => return Ok(None),
        };

        // Reuse document_highlights: every occurrence of the symbol is a linked range.
        let highlights = document_highlights(&source, &doc, position);
        if highlights.is_empty() {
            return Ok(None);
        }

        // Bail when the cursor's word isn't itself one of the highlight
        // ranges. `document_highlights` resolves the cursor to a word and
        // walks the AST for occurrences of that name; if the cursor sits in
        // a comment or string literal that happens to share a word with a
        // real identifier, the AST occurrences would still come back and
        // entering linked-edit mode would silently mirror unrelated ranges.
        // Comparing against `word_range_at` (rather than a contains check)
        // also accepts the half-open right boundary — a common cursor
        // position right after typing the name.
        if !highlights.iter().any(|h| h.range == cursor_word_range) {
            return Ok(None);
        }

        // Scope class-member rewrites so that two unrelated classes sharing
        // a method/property/const name aren't linked together — but keep
        // legitimate call sites at module scope (`$obj->bar()` outside any
        // class). The rule: drop highlights that fall inside *another*
        // class than the cursor's. Highlights inside the cursor's class
        // and at module scope (outside every class) are preserved.
        // Class declarations themselves (cursor on the class header) stay
        // global so renaming a class spans the whole file.
        let scope_to_class = !is_variable
            && crate::type_map::enclosing_class_at(&source, &doc, position).as_deref()
                != Some(word.as_str());
        let other_class_ranges: Vec<Range> = if scope_to_class {
            let cursor_class = crate::type_map::enclosing_class_range_at(&doc, position);
            crate::type_map::collect_all_class_ranges(&doc)
                .into_iter()
                .filter(|r| Some(*r) != cursor_class)
                .collect()
        } else {
            Vec::new()
        };
        let ranges: Vec<Range> = highlights
            .into_iter()
            .map(|h| h.range)
            .filter(|r| !other_class_ranges.iter().any(|ocr| range_within(*r, *ocr)))
            .collect();
        if ranges.is_empty() {
            return Ok(None);
        }

        // Variables include the leading `$` in their range, so the pattern
        // must require it; for everything else (class/function/method names)
        // a `$` would produce invalid PHP. The Unicode range covers the
        // full BMP so that PHP identifiers using non-Latin alphabets
        // (CJK, Cyrillic, Greek, …) round-trip through linked-mode
        // typing rather than being rejected by the regex.
        let word_pattern = if is_variable {
            r"\$[a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*".to_string()
        } else {
            r"[a-zA-Z_\u00A0-\uFFFF][a-zA-Z0-9_\u00A0-\uFFFF]*".to_string()
        };
        Ok(Some(LinkedEditingRanges {
            ranges,
            word_pattern: Some(word_pattern),
        }))
    }

    async fn goto_implementation(
        &self,
        params: tower_lsp::lsp_types::request::GotoImplementationParams,
    ) -> Result<Option<tower_lsp::lsp_types::request::GotoImplementationResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        let imports = self.file_imports(uri);
        let word = crate::util::word_at_position(&source, position).unwrap_or_default();
        let fqn = imports.get(&word).map(|s| s.as_str());
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.docs_for(&self.open_urls());
        let mut locs = find_implementations(&word, fqn, &open_docs);
        if locs.is_empty() {
            // Second pass: background files via the salsa-memoized workspace
            // aggregate's `subtypes_of` reverse map (line-only positions).
            let wi = self.docs.get_workspace_index_salsa();
            locs = find_implementations_from_workspace(&word, fqn, &wi);
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
        let source = self.get_open_text(uri).unwrap_or_default();
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.docs_for(&self.open_urls());
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
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let analysis = self.docs.cached_analysis(uri);
        // First pass: open-file ParsedDocs give accurate character positions.
        let open_docs = self.docs.docs_for(&self.open_urls());
        let mut results =
            goto_type_definition(&source, &doc, analysis.as_deref(), &open_docs, position);

        // If no results from first pass, try background files via FileIndex (line-only positions).
        if results.is_empty() {
            let all_indexes = self.docs.all_indexes();
            results = goto_type_definition_from_index(
                &source,
                &doc,
                analysis.as_deref(),
                &all_indexes,
                position,
            );
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
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.get_open_text(uri).unwrap_or_default();
        // Phase J: use the salsa-memoized aggregate's `classes_by_name` map.
        let wi = self.docs.get_workspace_index_salsa();
        Ok(prepare_type_hierarchy_from_workspace(&source, &wi, position).map(|item| vec![item]))
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        // Phase J: resolve parents via the aggregate's `classes_by_name` map.
        let wi = self.docs.get_workspace_index_salsa();
        let result = supertypes_of_from_workspace(&params.item, &wi);
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
        // Phase J: O(matches) lookup via the aggregate's `subtypes_of` map.
        let wi = self.docs.get_workspace_index_salsa();
        let result = subtypes_of_from_workspace(&params.item, &wi);
        Ok(if result.is_empty() {
            None
        } else {
            Some(result)
        })
    }

    async fn code_lens(&self, params: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        let uri = &params.text_document.uri;
        let doc = match self.get_doc(uri) {
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
        let doc = match self.get_doc(uri) {
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
        let source = self.get_open_text(uri).unwrap_or_default();
        Ok(format_document(&source))
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        Ok(format_range(&source, params.range))
    }

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document_position.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
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
    }

    async fn will_rename_files(&self, params: RenameFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
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
                self.index_if_not_open(new_uri, &text);
            }
        }
    }

    // ── File-create notifications ────────────────────────────────────────────

    async fn will_create_files(&self, params: CreateFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
        let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file in &params.files {
            let Ok(uri) = Url::parse(&file.uri) else {
                continue;
            };
            // Check the extension from the URI path so this works on Windows
            // where to_file_path() fails for drive-less URIs (e.g. file:///foo.php).
            if !uri.path().ends_with(".php") {
                continue;
            }

            let stub = if let Ok(path) = uri.to_file_path()
                && let Some(fqn) = psr4.file_to_fqn(&path)
            {
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
                self.index_if_not_open(uri, &text);
            }
        }
        send_refresh_requests(&self.client).await;
    }

    // ── File-delete notifications ────────────────────────────────────────────

    /// Before a file is deleted, return workspace edits that remove every
    /// `use` import referencing its PSR-4 class name.
    async fn will_delete_files(&self, params: DeleteFilesParams) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
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
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let imports = self.file_imports(uri);
        Ok(moniker_at(&source, &doc, position, &imports).map(|m| vec![m]))
    }

    // ── Inline values ────────────────────────────────────────────────────────

    async fn inline_value(&self, params: InlineValueParams) -> Result<Option<Vec<InlineValue>>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
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
        let source = self.get_open_text(uri).unwrap_or_default();

        let parse_diags = self.get_parse_diagnostics(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => {
                // Even if document not fully indexed, compute result_id for parse diagnostics
                let _version = self
                    .open_files
                    .all_with_diagnostics()
                    .iter()
                    .find(|(u, _, _)| u == uri)
                    .and_then(|(_, _, v)| *v)
                    .unwrap_or(1);
                let result_id = compute_diagnostic_result_id(&parse_diags, uri.as_str());
                return Ok(DocumentDiagnosticReportResult::Report(
                    DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                        related_documents: None,
                        full_document_diagnostic_report: FullDocumentDiagnosticReport {
                            result_id: Some(result_id),
                            items: parse_diags,
                        },
                    }),
                ));
            }
        };
        let (diag_cfg, php_version) = {
            let cfg = self.config.load();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };
        // Note: php_version could be used for version-specific diagnostics in the future
        let _ = php_version;

        // Phase I: salsa Pass-2 is CPU-bound; run off the async executor.
        let docs = Arc::clone(&self.docs);
        let uri_owned = uri.clone();
        let diag_cfg_sem = diag_cfg.clone();
        let sem_diags = tokio::task::spawn_blocking(move || {
            docs.get_semantic_issues_salsa(&uri_owned)
                .map(|issues| {
                    crate::semantic_diagnostics::issues_to_diagnostics(
                        &issues,
                        &uri_owned,
                        &diag_cfg_sem,
                    )
                })
                .unwrap_or_default()
        })
        .await
        .map_err(|e| {
            use std::borrow::Cow;
            tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::InternalError,
                message: Cow::Owned(format!("diagnostic analysis failed: {}", e)),
                data: None,
            }
        })?;

        let items = merge_file_diagnostics(
            parse_diags,
            duplicate_declaration_diagnostics(&source, &doc, &diag_cfg),
            sem_diags,
        );

        // Generate stable result_id for caching
        let _version = self
            .open_files
            .all_with_diagnostics()
            .iter()
            .find(|(u, _, _)| u == uri)
            .and_then(|(_, _, v)| *v)
            .unwrap_or(1);
        let result_id = compute_diagnostic_result_id(&items, uri.as_str());

        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: Some(result_id),
                    items,
                },
            }),
        ))
    }

    async fn workspace_diagnostic(
        &self,
        params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let all_parse_diags = self.all_open_files_with_diagnostics();
        let (diag_cfg, php_version) = {
            let cfg = self.config.load();
            (cfg.diagnostics.clone(), cfg.php_version.clone())
        };

        // Note: php_version could be used for version-specific diagnostics in the future
        let _ = php_version;

        // Build a URI→result_id lookup from the client's cached state.
        // Per LSP §3.17.7: files present in this map with a matching result_id
        // should return Unchanged; all others return Full.
        // Duplicate URIs: last-wins (HashMap collect). Clients shouldn't send duplicates,
        // but if they do the last entry wins — safe and simple.
        let previous_map: std::collections::HashMap<Url, String> = params
            .previous_result_ids
            .into_iter()
            .map(|p| (p.uri, p.value))
            .collect();

        // Phase I: each file's semantic issues flow through the salsa
        // `semantic_issues` query. The memo is shared with `did_open` /
        // `did_change` / `document_diagnostic` / `code_action`, so repeated
        // workspace-diagnostic pulls reuse prior analysis. The first pull on
        // a cold workspace still walks every file's `StatementsAnalyzer` —
        // run the whole sweep on the blocking pool so the async runtime
        // stays responsive.
        let docs = Arc::clone(&self.docs);
        let diag_cfg_sweep = diag_cfg.clone();
        let items = tokio::task::spawn_blocking(move || {
            all_parse_diags
                .into_iter()
                .filter_map(|(uri, parse_diags, version)| {
                    let doc = docs.get_doc_salsa(&uri)?;

                    let source = doc.source().to_string();
                    let sem_diags = docs
                        .get_semantic_issues_salsa(&uri)
                        .map(|issues| {
                            crate::semantic_diagnostics::issues_to_diagnostics(
                                &issues,
                                &uri,
                                &diag_cfg_sweep,
                            )
                        })
                        .unwrap_or_default();
                    let all_diags = merge_file_diagnostics(
                        parse_diags,
                        duplicate_declaration_diagnostics(&source, &doc, &diag_cfg_sweep),
                        sem_diags,
                    );

                    let result_id = compute_diagnostic_result_id(&all_diags, uri.as_str());

                    // Per LSP §3.17.7: return Unchanged only when the client already has
                    // this exact result_id cached for this URI; otherwise return Full.
                    if previous_map.get(&uri) == Some(&result_id) {
                        Some(WorkspaceDocumentDiagnosticReport::Unchanged(
                            WorkspaceUnchangedDocumentDiagnosticReport {
                                uri,
                                version,
                                unchanged_document_diagnostic_report:
                                    UnchangedDocumentDiagnosticReport { result_id },
                            },
                        ))
                    } else {
                        Some(WorkspaceDocumentDiagnosticReport::Full(
                            WorkspaceFullDocumentDiagnosticReport {
                                uri,
                                version,
                                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                                    result_id: Some(result_id),
                                    items: all_diags,
                                },
                            },
                        ))
                    }
                })
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| {
            use std::borrow::Cow;
            tower_lsp::jsonrpc::Error {
                code: tower_lsp::jsonrpc::ErrorCode::InternalError,
                message: Cow::Owned(format!("workspace_diagnostic analysis failed: {}", e)),
                data: None,
            }
        })?;

        Ok(WorkspaceDiagnosticReportResult::Report(
            WorkspaceDiagnosticReport { items },
        ))
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let source = self.get_open_text(uri).unwrap_or_default();
        let doc = match self.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let other_docs = self.docs.other_docs(uri, &self.open_urls());

        // Phase I: read semantic issues through the salsa query. The result
        // is memoized across did_open/did_change/document_diagnostic, so
        // code_action usually hits the memo instead of rerunning analysis.
        // On a memo miss (e.g. code-action fires before did_open finishes),
        // the analyzer runs — park that on the blocking pool so the async
        // runtime doesn't stall.
        let diag_cfg = self.config.load().diagnostics.clone();
        let docs_sem = Arc::clone(&self.docs);
        let uri_sem = uri.clone();
        let diag_cfg_sem = diag_cfg.clone();
        let sem_diags = tokio::task::spawn_blocking(move || {
            docs_sem
                .get_semantic_issues_salsa(&uri_sem)
                .map(|issues| {
                    crate::semantic_diagnostics::issues_to_diagnostics(
                        &issues,
                        &uri_sem,
                        &diag_cfg_sem,
                    )
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();

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

        // Defer edit computation to code_action_resolve so the menu renders
        // instantly; the client fetches the full edit only for the selected item.
        for tag in DEFERRED_ACTION_TAGS {
            actions.extend(defer_actions(
                self.generate_deferred_actions(tag, &source, &doc, params.range, uri),
                tag,
                uri,
                params.range,
            ));
        }

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

        let source = self.get_open_text(&uri).unwrap_or_default();
        let doc = match self.get_doc(&uri) {
            Some(d) => d,
            None => return Ok(item),
        };

        let candidates = self.generate_deferred_actions(&kind_tag, &source, &doc, range, &uri);

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
