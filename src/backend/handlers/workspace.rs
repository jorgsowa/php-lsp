use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::notification::Progress as ProgressNotification;
use tower_lsp_server::ls_types::request::WorkDoneProgressCreate;
use tower_lsp_server::ls_types::*;

use crate::analysis::semantic_tokens::legend;
use crate::editing::file_rename::{
    class_case_variants, delete_use_in_source, docblock_reference_edits, moved_file_edits,
    namespace_of, qualified_reference_edit, same_namespace_import_edit, use_edits_in_source,
};
use crate::index::workspace_scan::{scan_workspace, send_refresh_requests};
use crate::lang::autoload::Psr4Map;
use crate::lang::config::LspConfig;
use crate::text::fqn_short_name;

use super::super::helpers::{has_indexed_extension, indexed_file_op, indexed_file_or_folder_op};
use super::super::{Backend, IndexReadyNotification};

/// Non-PHP files whose changes reload the PSR-4 map or the Laravel index.
const PROJECT_METADATA_GLOBS: &[&str] = &[
    "**/composer.json",
    "**/vendor/composer/installed.json",
    "**/artisan",
    "**/.env",
    "**/.env.example",
    "**/lang/*.json",
    "**/mix-manifest.json",
    "**/build/manifest.json",
];

/// Files whose creation or deletion reloads the Laravel asset index.
const PUBLIC_ASSETS_GLOB: &str = "**/public/**";

impl Backend {
    pub(crate) async fn handle_initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResult> {
        {
            let mut roots: Vec<PathBuf> = params
                .workspace_folders
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .filter_map(|f| f.uri.to_file_path().map(|c| c.into_owned()))
                .collect();
            #[allow(deprecated)]
            let root_uri = params.root_uri.as_ref();
            if roots.is_empty()
                && let Some(path) = root_uri.and_then(|u| u.to_file_path().map(|c| c.into_owned()))
            {
                roots.push(path);
            }
            self.root_paths.store(Arc::new(roots));
        }

        self.client_capabilities
            .store(Arc::new(params.capabilities.clone()));
        self.open_files.set_client_pulls_diagnostics(
            params
                .capabilities
                .text_document
                .as_ref()
                .is_some_and(|td| td.diagnostic.is_some()),
        );

        {
            let opts = params.initialization_options.as_ref();
            let roots = self.root_paths.load_full();
            let file_cfg = crate::lang::autoload::load_project_config_json(&roots);

            if matches!(file_cfg, Some(serde_json::Value::Null)) {
                self.client
                    .log_message(
                        tower_lsp_server::ls_types::MessageType::WARNING,
                        "php-lsp: .php-lsp.json contains invalid JSON — ignoring",
                    )
                    .await;
            }

            if let Some(serde_json::Value::Object(ref obj)) = file_cfg
                && let Some(ver) = obj.get("phpVersion").and_then(|v| v.as_str())
                && !crate::lang::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp_server::ls_types::MessageType::WARNING,
                        format!(
                            "php-lsp: .php-lsp.json unsupported phpVersion {ver:?} — valid values: {}",
                            crate::lang::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }

            if let Some(serde_json::Value::Object(ref obj)) = file_cfg {
                for key in crate::lang::config::EDITOR_ONLY_KEYS {
                    if obj.contains_key(*key) {
                        self.client
                            .log_message(
                                tower_lsp_server::ls_types::MessageType::WARNING,
                                format!(
                                    "php-lsp: ignoring {key:?} in .php-lsp.json — \
                                     set it in editor settings instead"
                                ),
                            )
                            .await;
                    }
                }
            }

            if let Some(ver) = opts
                .and_then(|o| o.get("phpVersion"))
                .and_then(|v| v.as_str())
                && !crate::lang::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp_server::ls_types::MessageType::WARNING,
                        format!(
                            "php-lsp: unsupported phpVersion {ver:?} — valid values: {}",
                            crate::lang::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }

            let file_obj = file_cfg.as_ref().filter(|v| v.is_object());
            let merged = LspConfig::merge_project_configs(file_obj, opts);
            let mut cfg = LspConfig::from_value(&merged);

            let roots_for_psr4 = (*roots).clone();
            let psr4_extensions = cfg.php_file_extensions();
            let roots_for_ver = (*roots).clone();
            let explicit_version = cfg.php_version.clone();
            let (psr4_result, ver_result) = tokio::join!(
                tokio::task::spawn_blocking(move || {
                    let mut merged = Psr4Map::empty();
                    for root in &roots_for_psr4 {
                        merged.extend(Psr4Map::load_with_extensions(root, psr4_extensions.clone()));
                    }
                    merged
                }),
                tokio::task::spawn_blocking(move || {
                    crate::lang::autoload::resolve_php_version_from_roots(
                        &roots_for_ver,
                        explicit_version.as_deref(),
                    )
                }),
            );
            if let Ok(psr4) = psr4_result {
                self.psr4.store(Arc::new(psr4));
            }
            let (ver, source) = ver_result
                .unwrap_or_else(|_| (crate::lang::autoload::PHP_8_5.to_string(), "default"));
            self.client
                .log_message(
                    tower_lsp_server::ls_types::MessageType::INFO,
                    format!("php-lsp: using PHP {ver} ({source})"),
                )
                .await;
            let ver = if source != "set by editor"
                && !crate::lang::autoload::is_valid_php_version(&ver)
            {
                let clamped = crate::lang::autoload::clamp_php_version(&ver);
                self.client
                    .show_message(
                        tower_lsp_server::ls_types::MessageType::WARNING,
                        format!(
                            "php-lsp: detected PHP {ver} is outside the supported range \
                                 ({}); using PHP {clamped} for analysis",
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
            self.docs.set_taint_analysis(cfg.diagnostics.taint);
            self.config.store(Arc::new(cfg));
        }

        let feat = self.config.load().features.clone();
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        will_save: Some(true),
                        will_save_wait_until: Some(feat.formatting),
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
                        "\\".to_string(),
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
                        code_action_kinds: Some(vec![
                            CodeActionKind::QUICKFIX,
                            CodeActionKind::REFACTOR,
                            CodeActionKind::REFACTOR_EXTRACT,
                            CodeActionKind::REFACTOR_INLINE,
                            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                        ]),
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
                    resolve_provider: Some(false),
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
                    file_operations: {
                        let indexed_extensions = self.config.load().indexed_extensions.clone();
                        Some(WorkspaceFileOperationsServerCapabilities {
                            will_rename: Some(indexed_file_or_folder_op(&indexed_extensions)),
                            did_rename: Some(indexed_file_op(&indexed_extensions)),
                            will_create: Some(indexed_file_op(&indexed_extensions)),
                            did_create: Some(indexed_file_op(&indexed_extensions)),
                            will_delete: Some(indexed_file_op(&indexed_extensions)),
                            did_delete: Some(indexed_file_op(&indexed_extensions)),
                        })
                    },
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

    pub(crate) async fn handle_initialized(&self, _params: InitializedParams) {
        let roots: Vec<PathBuf> = (**self.root_paths.load()).clone();
        if roots.is_empty() {
            // No workspace root (single-file mode): there is nothing to scan,
            // so the index is trivially complete. Set this before any `.await`
            // below — otherwise a client slow to ack `client/registerCapability`
            // would leave `is_index_ready` false (holding back UndefinedClass
            // and friends, see `compute_open_file_diagnostics`) for however
            // long that ack takes, with no scan in flight to ever flip it.
            self.docs.mark_index_ready();
        }

        let php_selector = serde_json::json!([{"language": "php"}]);
        let watched_globs: Vec<serde_json::Value> = self
            .config
            .load()
            .indexed_extensions
            .iter()
            .map(|ext| format!("**/*.{ext}"))
            .chain(PROJECT_METADATA_GLOBS.iter().map(|g| g.to_string()))
            .map(|glob| serde_json::json!({"globPattern": glob}))
            .chain(std::iter::once(serde_json::json!({
                "globPattern": PUBLIC_ASSETS_GLOB,
                "kind": WatchKind::Create.bits() | WatchKind::Delete.bits(),
            })))
            .collect();
        let mut registrations = vec![
            Registration {
                id: "php-lsp-file-watcher".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(serde_json::json!({ "watchers": watched_globs })),
            },
            Registration {
                id: "php-lsp-config-change".to_string(),
                method: "workspace/didChangeConfiguration".to_string(),
                register_options: Some(serde_json::json!({"section": "php-lsp"})),
            },
        ];
        // `lsp-types` 0.94 has no static `typeHierarchyProvider` capability
        // field, so this is the only way to advertise support — but a
        // dynamic registration is only meaningful to a client that declared
        // it understands one; sending it regardless would ask a client with
        // no such capability to route a method it never agreed to handle.
        let supports_type_hierarchy_registration = self
            .client_capabilities
            .load()
            .text_document
            .as_ref()
            .and_then(|td| td.type_hierarchy.as_ref())
            .and_then(|th| th.dynamic_registration)
            .unwrap_or(false);
        if supports_type_hierarchy_registration {
            registrations.push(Registration {
                id: "php-lsp-type-hierarchy".to_string(),
                method: "textDocument/prepareTypeHierarchy".to_string(),
                register_options: Some(serde_json::json!({"documentSelector": php_selector})),
            });
        }
        self.client.register_capability(registrations).await.ok();

        if !roots.is_empty() {
            // `initialize` already loaded the PSR-4 map. The Laravel index
            // walks and parses project files, so it loads beside the scan.
            let laravel_load = {
                let roots = roots.clone();
                let laravel = Arc::clone(&self.laravel);
                tokio::task::spawn_blocking(move || {
                    let index = crate::laravel::LaravelIndex::load_first(&roots);
                    let is_laravel = index.is_laravel;
                    laravel.store(Arc::new(index));
                    is_laravel
                })
            };

            let token = NumberOrString::String("php-lsp/indexing".to_string());
            self.client
                .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                    token: token.clone(),
                })
                .await
                .ok();

            let extensions = self.config.load().php_file_extensions();
            let (
                exclude_paths,
                include_paths,
                max_indexed_files,
                debug,
                cache_path,
                warm_analysis,
                flush_interval_ms,
                stub_dirs,
            ) = {
                let cfg = self.config.load();
                let mut exclude = cfg.exclude_paths.clone();
                if !cfg.index_vendor && !exclude.iter().any(|p| p == "vendor" || p == "vendor/") {
                    exclude.push("vendor/".to_string());
                }
                (
                    exclude,
                    cfg.include_paths.clone(),
                    cfg.max_indexed_files,
                    cfg.debug,
                    cfg.cache_path.clone(),
                    cfg.warm_analysis,
                    cfg.flush_interval_ms,
                    cfg.stub_dirs.clone(),
                )
            };

            // Attach the on-disk analysis cache before any task can build an
            // `AnalysisSession`: `analysis_session()` caches its session per
            // PHP version on first build, so a session built before the cache
            // dir is known would permanently pin an in-memory-only session —
            // silently dropping every warm sweep's flush for the server's
            // lifetime. Must run before the prewarm task below is spawned.
            let first_root_cache = if let Some(ref p) = cache_path {
                Some(crate::index::cache::WorkspaceCache::with_dir(p.clone()))
            } else {
                crate::index::cache::WorkspaceCache::new(&roots[0])
            };

            // Same "before any session build" constraint as the cache dir
            // above — resolve relative entries against the first workspace
            // root before handing them to the session builder.
            if !stub_dirs.is_empty() {
                let resolved: Vec<PathBuf> = stub_dirs
                    .iter()
                    .map(|d| {
                        let p = PathBuf::from(d);
                        if p.is_absolute() { p } else { roots[0].join(p) }
                    })
                    .collect();
                if self.docs.set_user_stub_dirs(resolved) {
                    self.remirror_open_files();
                }
            }

            if let Some(ref c) = first_root_cache
                && self
                    .docs
                    .set_session_cache_dir(c.cache_dir().join("session"))
            {
                self.remirror_open_files();
            }

            // Postings staged by an in-progress warm sweep or an on-demand
            // reference-query freshness pass only reach disk on sweep
            // completion or clean shutdown (see `flush_analysis_cache`). A
            // 15K-file workspace's first sweep can run far longer than an
            // editor session lasts, and an unclean exit (crash, kill) skips
            // `shutdown` entirely — so without this, a still-warming
            // workspace pays its cold-analysis cost again on every restart.
            // `flush_analysis_cache` is a no-op unless something changed
            // since the last flush, so this loop costs nothing once the
            // workspace is fully warm and idle.
            let periodic_flush_docs = Arc::clone(&self.docs);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(flush_interval_ms)).await;
                    let docs = Arc::clone(&periodic_flush_docs);
                    let _ = tokio::task::spawn_blocking(move || docs.flush_analysis_cache()).await;
                }
            });

            let warm_docs = Arc::clone(&self.docs);
            tokio::task::spawn_blocking(move || {
                warm_docs.with_session(|_| ());
            });

            let docs = Arc::clone(&self.docs);
            let open_files = self.open_files.clone();
            let client = self.client.clone();
            let psr4 = self.psr4.clone();
            let config = Arc::clone(&self.config);
            tokio::spawn(async move {
                client
                    .send_notification::<ProgressNotification>(ProgressParams {
                        token: token.clone(),
                        value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                            WorkDoneProgressBegin {
                                title: "php-lsp: indexing workspace".to_string(),
                                cancellable: Some(false),
                                message: None,
                                percentage: Some(0),
                            },
                        )),
                    })
                    .await;

                // Channel for per-chunk progress from scan_workspace.
                // The receiver task sends $/progress Report notifications as chunks
                // complete; the sender is cloned into each scan_workspace call and
                // dropped here after all roots have been scanned to close the channel.
                let (progress_tx, mut progress_rx) =
                    tokio::sync::mpsc::unbounded_channel::<(usize, usize)>();
                tokio::spawn({
                    let client = client.clone();
                    let token = token.clone();
                    async move {
                        while let Some((done, total_files)) = progress_rx.recv().await {
                            let pct = (done * 100).checked_div(total_files).unwrap_or(100) as u32;
                            client
                                .send_notification::<ProgressNotification>(ProgressParams {
                                    token: token.clone(),
                                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::Report(
                                        WorkDoneProgressReport {
                                            cancellable: Some(false),
                                            message: Some(format!("{done}/{total_files}")),
                                            percentage: Some(pct),
                                        },
                                    )),
                                })
                                .await;
                        }
                    }
                });

                let scan_start = std::time::Instant::now();
                let mut total = 0usize;
                for root in &roots {
                    total += scan_workspace(
                        root.clone(),
                        Arc::clone(&docs),
                        open_files.clone(),
                        &roots,
                        &exclude_paths,
                        &include_paths,
                        &extensions,
                        max_indexed_files,
                        Some(progress_tx.clone()),
                    )
                    .await;
                }
                // Drop the sender to close the channel; the receiver task exits once
                // it has processed all remaining messages.
                drop(progress_tx);
                let elapsed = scan_start.elapsed();
                let elapsed_s = elapsed.as_secs_f64();

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
                        format!("php-lsp: indexed {total} files in {elapsed_s:.1} s"),
                    )
                    .await;

                if debug {
                    let root_list = roots
                        .iter()
                        .map(|r| r.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let ns_count = psr4.load().project_namespace_count();
                    client
                        .log_message(
                            MessageType::INFO,
                            format!(
                                "php-lsp: debug: {ns_count} PSR-4 namespaces | roots: {root_list}"
                            ),
                        )
                        .await;
                }

                send_refresh_requests(&client).await;

                let salsa_docs = Arc::clone(&docs);
                // Build the workspace aggregate and replay disk-cached index
                // postings/subtype edges BEFORE signaling readiness: a query
                // fired at `indexReady` otherwise races both — it rebuilds
                // the aggregate itself (the mirror writes invalidated the
                // scan-time build) and, on a returning session, sees none of
                // the replayed freshness marks, paying the whole-workspace
                // defs walk the replay exists to avoid. Both steps are
                // no-ops-per-file on a first-ever run, and neither analyzes:
                // the reference-warm phase that does is deliberately left
                // behind readiness (see below).
                let (workspace_index_elapsed_s, warm_start_elapsed_s, untrusted) = {
                    let pre = Arc::clone(&salsa_docs);
                    let (workspace_index_elapsed, warm_start_elapsed, untrusted) =
                        tokio::task::spawn_blocking(move || {
                            let workspace_index_start = std::time::Instant::now();
                            pre.get_workspace_index_salsa();
                            let workspace_index_elapsed = workspace_index_start.elapsed();

                            let warm_start_begin = std::time::Instant::now();
                            let untrusted = pre.warm_start_indexes();
                            let warm_start_elapsed = warm_start_begin.elapsed();

                            (workspace_index_elapsed, warm_start_elapsed, untrusted)
                        })
                        .await
                        .unwrap_or((
                            std::time::Duration::ZERO,
                            std::time::Duration::ZERO,
                            Vec::new(),
                        ));
                    (
                        workspace_index_elapsed.as_secs_f64(),
                        warm_start_elapsed.as_secs_f64(),
                        untrusted,
                    )
                };
                if debug {
                    client
                        .log_message(
                            MessageType::INFO,
                            format!(
                                "php-lsp: debug: pre-ready workspace_index={workspace_index_elapsed_s:.3}s \
                                 warm_start={warm_start_elapsed_s:.3}s total={:.3}s",
                                workspace_index_elapsed_s + warm_start_elapsed_s
                            ),
                        )
                        .await;
                }
                let is_laravel = laravel_load.await.unwrap_or(false);
                docs.mark_index_ready();
                drop(docs);
                client.send_notification::<IndexReadyNotification>(()).await;
                // Files opened while the scan was still running may have been
                // published with UndefinedClass/UndefinedFunction/etc. held
                // back (see `compute_open_file_diagnostics`); push clients
                // have no other trigger to pick up the now-resolvable ones.
                tokio::spawn(super::super::republish_after_index_ready(
                    client.clone(),
                    Arc::clone(&salsa_docs),
                    open_files.clone(),
                    config.load().diagnostics.clone(),
                    is_laravel,
                ));

                // Reference-warm phase, behind readiness. Mir's returned
                // warm-start priority set always runs as a cheap performance
                // warm-up, while the ambient project sweep only joins the
                // queue when `warmAnalysis` is on. Reporting work-done
                // progress keeps the seconds it costs visible without holding
                // a request or the readiness gate.
                let phase_open = open_files.urls();
                drop(tokio::spawn(async move {
                    let token = NumberOrString::String("php-lsp/warming".to_string());
                    client
                        .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                            token: token.clone(),
                        })
                        .await
                        .ok();
                    client
                        .send_notification::<ProgressNotification>(ProgressParams {
                            token: token.clone(),
                            value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                                WorkDoneProgressBegin {
                                    title: "php-lsp: warming reference index".to_string(),
                                    cancellable: Some(false),
                                    message: None,
                                    percentage: Some(0),
                                },
                            )),
                        })
                        .await;
                    let (phase_tx, mut phase_rx) =
                        tokio::sync::mpsc::unbounded_channel::<(u32, u32)>();
                    let reporter = tokio::spawn({
                        let client = client.clone();
                        let token = token.clone();
                        async move {
                            while let Some((done, total_files)) = phase_rx.recv().await {
                                let pct = (done * 100) / total_files.max(1);
                                client
                                    .send_notification::<ProgressNotification>(ProgressParams {
                                        token: token.clone(),
                                        value: ProgressParamsValue::WorkDone(
                                            WorkDoneProgress::Report(WorkDoneProgressReport {
                                                cancellable: Some(false),
                                                message: Some(format!(
                                                    "warming {done}/{total_files} files"
                                                )),
                                                percentage: Some(pct),
                                            }),
                                        ),
                                    })
                                    .await;
                            }
                        }
                    });
                    let warm_phase_begin = std::time::Instant::now();
                    tokio::task::spawn_blocking(move || {
                        salsa_docs.warm_references_phase(
                            untrusted,
                            &phase_open,
                            warm_analysis,
                            Some(phase_tx),
                        );
                    })
                    .await
                    .ok();
                    let warm_phase_elapsed_s = warm_phase_begin.elapsed().as_secs_f64();
                    let _ = reporter.await;
                    client
                        .send_notification::<ProgressNotification>(ProgressParams {
                            token,
                            value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(
                                WorkDoneProgressEnd {
                                    message: Some("reference index warm".to_string()),
                                },
                            )),
                        })
                        .await;
                    if debug {
                        client
                            .log_message(
                                MessageType::INFO,
                                format!("php-lsp: debug: warm_phase={warm_phase_elapsed_s:.3}s"),
                            )
                            .await;
                    }
                }));
            });
        }

        self.client
            .log_message(
                MessageType::INFO,
                format!("php-lsp {} ready", env!("CARGO_PKG_VERSION")),
            )
            .await;
    }

    pub(crate) async fn handle_will_rename_files(
        &self,
        params: RenameFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
        let mut merged_changes: std::collections::HashMap<Uri, Vec<TextEdit>> =
            std::collections::HashMap::new();

        // A folder rename moves every indexed file under it.
        let workspace_files = self.docs.workspace_file_paths();
        let mut renames: Vec<(String, String)> = Vec::new();
        for file_rename in &params.files {
            let old_dir = format!("{}/", file_rename.old_uri.trim_end_matches('/'));
            let new_dir = format!("{}/", file_rename.new_uri.trim_end_matches('/'));
            let children: Vec<(String, String)> = workspace_files
                .iter()
                .filter_map(|f| {
                    let rest = f.strip_prefix(old_dir.as_str())?;
                    Some((f.to_string(), format!("{new_dir}{rest}")))
                })
                .collect();
            if children.is_empty() {
                renames.push((file_rename.old_uri.clone(), file_rename.new_uri.clone()));
            } else {
                renames.extend(children);
            }
        }
        let moves: Vec<(Uri, String, String)> = renames
            .iter()
            .filter_map(|(old, new)| {
                let old_uri = old.parse::<Uri>().ok()?;
                let old_path = old_uri.to_file_path()?.into_owned();
                let new_path = new.parse::<Uri>().ok()?.to_file_path()?.into_owned();
                let old_fqn = psr4.file_to_fqn(&old_path)?;
                let new_fqn = psr4.file_to_fqn(&new_path)?;
                Some((old_uri, old_fqn, new_fqn))
            })
            .collect();
        let moved_fqns: std::sync::Arc<std::collections::HashMap<String, String>> =
            std::sync::Arc::new(
                moves
                    .iter()
                    .map(|(_, old, new)| {
                        (
                            old.trim_start_matches('\\').to_ascii_lowercase(),
                            new.trim_start_matches('\\').to_string(),
                        )
                    })
                    .collect(),
            );
        // Files changing namespace import what they need themselves.
        let moved_namespaces: std::sync::Arc<std::collections::HashMap<Uri, String>> =
            std::sync::Arc::new(
                moves
                    .iter()
                    .filter(|(_, old, new)| {
                        !namespace_of(old).eq_ignore_ascii_case(namespace_of(new))
                    })
                    .map(|(uri, _, new)| (uri.clone(), namespace_of(new).to_string()))
                    .collect(),
            );

        // `use` lines rewrite the full import path (the namespace may change,
        // not just the class name). Only importers of a moved FQN can carry
        // such a line, and the workspace index already records per-file
        // imports — no text scan, parse only matches. A file is handled once
        // for all moves, so members of one group use are edited together.
        let renamed: std::collections::HashMap<String, (String, String)> = moves
            .iter()
            .filter(|(_, old, new)| old != new)
            .map(|(_, old, new)| {
                let old = old.trim_start_matches('\\');
                (old.to_ascii_lowercase(), (old.to_string(), new.clone()))
            })
            .collect();
        if !renamed.is_empty() {
            let docs = std::sync::Arc::clone(&self.docs);
            let gate = std::sync::Arc::clone(&self.debug_gate);
            // Every importing file gets parsed and diffed; keep that off the
            // async runtime worker, same as the reference-lookup path below.
            let edits_by_uri = tokio::task::spawn_blocking(move || {
                gate.pass(super::super::debug_gate::GATE_WILL_RENAME_FILES);
                let importers: std::collections::HashSet<Uri> = renamed
                    .values()
                    .flat_map(|(old, _)| docs.files_importing(old))
                    .collect();
                let mut out = Vec::new();
                for uri in importers {
                    let Some(doc) = docs.get_doc_salsa(&uri) else {
                        continue;
                    };
                    let edits = use_edits_in_source(&doc, |fqn| {
                        renamed
                            .get(&fqn.to_ascii_lowercase())
                            .map(|(_, new)| new.clone())
                    });
                    if !edits.is_empty() {
                        out.push((uri, edits));
                    }
                }
                out
            })
            .await
            .unwrap_or_default();
            for (uri, edits) in edits_by_uri {
                merged_changes.entry(uri).or_default().extend(edits);
            }
        }

        for (old_uri, old_fqn, new_fqn) in moves {
            let old_uri = Some(old_uri);
            let old_short = fqn_short_name(&old_fqn).to_string();
            let new_short = fqn_short_name(&new_fqn).to_string();
            let ns_changed = !namespace_of(&old_fqn).eq_ignore_ascii_case(namespace_of(&new_fqn));
            if ns_changed && let Some(old_uri) = old_uri.clone() {
                let docs = std::sync::Arc::clone(&self.docs);
                let (old_fqn_task, new_fqn_task) = (old_fqn.clone(), new_fqn.clone());
                let moved = std::sync::Arc::clone(&moved_fqns);
                let edits = tokio::task::spawn_blocking(move || {
                    let doc = docs.get_doc_salsa(&old_uri)?;
                    let exists = |kind, fqn: &str| {
                        let name = match kind {
                            php_ast::UseKind::Normal => return docs.find_class_like(fqn).is_some(),
                            php_ast::UseKind::Function => mir_analyzer::Name::function(fqn),
                            php_ast::UseKind::Const => {
                                mir_analyzer::Name::GlobalConstant(fqn.into())
                            }
                            _ => return false,
                        };
                        docs.definition_location(&name).is_some()
                    };
                    let edits =
                        moved_file_edits(&doc, &old_fqn_task, &new_fqn_task, exists, |fqn| {
                            moved.get(&fqn.to_ascii_lowercase()).cloned()
                        });
                    Some((old_uri, edits))
                })
                .await
                .ok()
                .flatten();
                if let Some((uri, edits)) = edits {
                    merged_changes.entry(uri).or_default().extend(edits);
                }
            }

            // Also rename the declaration itself plus reference sites (type
            // hints, `new`, static calls, hierarchy clauses) from mir's
            // posting lists; `use` lines live under separate `use:` keys, so
            // this can't overlap the edits above. Unqualified references
            // that shared the old namespace gain an import of the new FQN.
            if old_short != new_short || ns_changed {
                let symbol =
                    mir_analyzer::Name::class(old_fqn.trim_start_matches('\\').to_string());
                let files: Vec<std::sync::Arc<str>> = self.docs.reference_candidate_files(&symbol);
                let docs = std::sync::Arc::clone(&self.docs);
                let candidate_files = files.clone();
                let old_fqn_task = old_fqn.clone();
                let locations = tokio::task::spawn_blocking(move || {
                    let (_interactive, cancel_rev) = docs.settled_write_rev_guard();
                    let mut variants = std::collections::HashSet::new();
                    for uri in files.iter().filter_map(|f| f.parse::<Uri>().ok()) {
                        if let Some(text) = docs.source_text(&uri) {
                            class_case_variants(&text, &old_fqn_task, &mut variants);
                        }
                    }
                    let mut locations =
                        docs.indexed_references(&symbol, &files, true, Some(cancel_rev))?;
                    for variant in variants {
                        let variant = mir_analyzer::Name::class(variant);
                        locations.extend(docs.indexed_references(
                            &variant,
                            &files,
                            true,
                            Some(cancel_rev),
                        )?);
                    }
                    Ok::<_, crate::document::document_store::ContentModified>(locations)
                })
                .await
                .unwrap_or_else(|_| Ok(Vec::new()))?;
                let docs = std::sync::Arc::clone(&self.docs);
                let old_short_task = old_short.clone();
                let new_short_task = new_short.clone();
                let old_fqn_task = old_fqn.clone();
                let new_fqn_task = new_fqn.clone();
                let moved_namespaces = std::sync::Arc::clone(&moved_namespaces);
                // Each reference site gets its declaring file parsed and its
                // rename range narrowed; same rationale as the `use`-edit
                // loop above.
                let edits = tokio::task::spawn_blocking(move || {
                    let mut out = Vec::new();
                    for loc in locations
                        .into_iter()
                        .filter_map(crate::navigation::references::session_tuple_to_location)
                    {
                        let Some(doc) = docs.get_doc_salsa(&loc.uri) else {
                            continue;
                        };
                        let Some(range) = crate::editing::rename::narrow_range_to_word_any_case(
                            doc.source(),
                            loc.range,
                            &old_short_task,
                        ) else {
                            continue;
                        };
                        let ns_after = moved_namespaces.get(&loc.uri).map(String::as_str);
                        if ns_changed
                            && let Some(edit) =
                                qualified_reference_edit(&doc, range, &new_fqn_task, ns_after)
                        {
                            out.push((loc.uri, edit));
                            continue;
                        }
                        if ns_changed
                            && !moved_namespaces.contains_key(&loc.uri)
                            && let Some(edit) = same_namespace_import_edit(
                                &doc,
                                range.start,
                                &old_fqn_task,
                                &new_fqn_task,
                            )
                        {
                            out.push((loc.uri.clone(), edit));
                        }
                        if old_short_task != new_short_task {
                            out.push((
                                loc.uri,
                                TextEdit {
                                    range,
                                    new_text: new_short_task.clone(),
                                },
                            ));
                        }
                    }
                    // Docblock types aren't in mir's reference postings.
                    for uri in candidate_files.iter().filter_map(|f| f.parse::<Uri>().ok()) {
                        let Some(doc) = docs.get_doc_salsa(&uri) else {
                            continue;
                        };
                        let ns_after = moved_namespaces.get(&uri).map(String::as_str);
                        for edit in
                            docblock_reference_edits(&doc, &old_fqn_task, &new_fqn_task, ns_after)
                        {
                            out.push((uri.clone(), edit));
                        }
                    }
                    out
                })
                .await
                .unwrap_or_default();
                for (uri, edit) in edits {
                    merged_changes.entry(uri).or_default().push(edit);
                }
            }
        }

        for edits in merged_changes.values_mut() {
            crate::editing::rename::sort_and_dedup_edits(edits);
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

    pub(crate) async fn handle_did_rename_files(&self, params: RenameFilesParams) {
        let indexed_extensions = self.config.load().indexed_extensions.clone();
        let mut any_indexed = false;
        for file_rename in &params.files {
            let old_is_indexed = has_indexed_extension(&file_rename.old_uri, &indexed_extensions);
            let new_is_indexed = has_indexed_extension(&file_rename.new_uri, &indexed_extensions);
            any_indexed |= old_is_indexed || new_is_indexed;

            if old_is_indexed && let Ok(old_uri) = file_rename.old_uri.parse::<Uri>() {
                self.docs.remove(&old_uri);
                // Clear diagnostics under the old path — same as did_delete_files —
                // or a client keeps showing them for a URI that no longer exists.
                self.open_files
                    .note_published(&old_uri, crate::backend::diagnostics_content_hash(&[]));
                self.client.publish_diagnostics(old_uri, vec![], None).await;
            }
            if new_is_indexed
                && let Ok(new_uri) = file_rename.new_uri.parse::<Uri>()
                && let Some(path) = new_uri.to_file_path()
                && let Ok(text) = tokio::fs::read(&path).await.map(crate::text::decode_source)
            {
                self.ingest_if_not_open(new_uri, &text);
            }
        }
        // Only ask clients to re-pull semantic tokens/diagnostics/etc. when a
        // renamed file was actually indexed-relevant — otherwise a rename of
        // an unrelated file (e.g. a doc renamed alongside the project) fans
        // out five refresh requests to every open editor for nothing.
        if any_indexed {
            send_refresh_requests(&self.client).await;
        }
    }

    pub(crate) async fn handle_will_create_files(
        &self,
        params: CreateFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
        let mut changes: std::collections::HashMap<Uri, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file in &params.files {
            let Ok(uri) = file.uri.parse::<Uri>() else {
                continue;
            };
            if !uri.path().as_str().ends_with(".php") {
                continue;
            }

            let stub = if let Some(path) = uri.to_file_path()
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

    pub(crate) async fn handle_did_create_files(&self, params: CreateFilesParams) {
        for file in &params.files {
            if let Ok(uri) = file.uri.parse::<Uri>()
                && let Some(path) = uri.to_file_path()
                && let Ok(text) = tokio::fs::read(&path).await.map(crate::text::decode_source)
            {
                self.ingest_if_not_open(uri, &text);
            }
        }
        send_refresh_requests(&self.client).await;
    }

    pub(crate) async fn handle_will_delete_files(
        &self,
        params: DeleteFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
        let mut merged_changes: std::collections::HashMap<Uri, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file in &params.files {
            let path = file
                .uri
                .parse::<Uri>()
                .ok()
                .and_then(|u| u.to_file_path().map(|c| c.into_owned()));
            let Some(path) = path else { continue };
            let Some(fqn) = psr4.file_to_fqn(&path) else {
                continue;
            };

            // Only importers of the FQN can carry a deletable `use` line;
            // the workspace index records per-file imports — no text scan.
            let docs = std::sync::Arc::clone(&self.docs);
            let gate = std::sync::Arc::clone(&self.debug_gate);
            let edits_by_uri = tokio::task::spawn_blocking(move || {
                gate.pass(super::super::debug_gate::GATE_WILL_DELETE_FILES);
                let mut out = Vec::new();
                for uri in docs.files_importing(&fqn) {
                    let Some(doc) = docs.get_doc_salsa(&uri) else {
                        continue;
                    };
                    let edits = delete_use_in_source(&doc, &fqn);
                    if !edits.is_empty() {
                        out.push((uri, edits));
                    }
                }
                out
            })
            .await
            .unwrap_or_default();
            for (uri, edits) in edits_by_uri {
                merged_changes.entry(uri).or_default().extend(edits);
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

    pub(crate) async fn handle_did_delete_files(&self, params: DeleteFilesParams) {
        for file in &params.files {
            if let Ok(uri) = file.uri.parse::<Uri>() {
                self.docs.remove(&uri);
                self.open_files
                    .note_published(&uri, crate::backend::diagnostics_content_hash(&[]));
                self.client.publish_diagnostics(uri, vec![], None).await;
            }
        }
        send_refresh_requests(&self.client).await;
    }
}
