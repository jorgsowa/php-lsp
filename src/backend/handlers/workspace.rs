use std::path::PathBuf;
use std::sync::Arc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::notification::Progress as ProgressNotification;
use tower_lsp::lsp_types::request::WorkDoneProgressCreate;
use tower_lsp::lsp_types::*;

use crate::analysis::semantic_tokens::legend;
use crate::editing::file_rename::{delete_use_in_source, use_edits_in_source};
use crate::index::workspace_scan::{scan_workspace, send_refresh_requests};
use crate::lang::autoload::Psr4Map;
use crate::lang::config::LspConfig;
use crate::text::fqn_short_name;

use super::super::helpers::php_file_op;
use super::super::{Backend, IndexReadyNotification};

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
            let file_cfg = crate::lang::autoload::load_project_config_json(&roots);

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
                && !crate::lang::autoload::is_valid_php_version(ver)
            {
                self.client
                    .log_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
                        format!(
                            "php-lsp: .php-lsp.json unsupported phpVersion {ver:?} — valid values: {}",
                            crate::lang::autoload::SUPPORTED_PHP_VERSIONS.join(", ")
                        ),
                    )
                    .await;
            }

            if let Some(ver) = opts
                .and_then(|o| o.get("phpVersion"))
                .and_then(|v| v.as_str())
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
            let merged = LspConfig::merge_project_configs(file_obj, opts);
            let mut cfg = LspConfig::from_value(&merged);

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
                    tower_lsp::lsp_types::MessageType::INFO,
                    format!("php-lsp: using PHP {ver} ({source})"),
                )
                .await;
            let ver = if source != "set by editor"
                && !crate::lang::autoload::is_valid_php_version(&ver)
            {
                let clamped = crate::lang::autoload::clamp_php_version(&ver);
                self.client
                    .show_message(
                        tower_lsp::lsp_types::MessageType::WARNING,
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
                    file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                        will_rename: Some(php_file_op()),
                        did_rename: Some(php_file_op()),
                        will_create: Some(php_file_op()),
                        did_create: Some(php_file_op()),
                        will_delete: Some(php_file_op()),
                        did_delete: Some(php_file_op()),
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

    pub(crate) async fn handle_initialized(&self, _params: InitializedParams) {
        let php_selector = serde_json::json!([{"language": "php"}]);
        let registrations = vec![
            Registration {
                id: "php-lsp-file-watcher".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(serde_json::json!({
                    "watchers": [{"globPattern": "**/*.php"}]
                })),
            },
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
            self.laravel
                .store(Arc::new(crate::laravel::LaravelIndex::load(&roots[0])));

            let token = NumberOrString::String("php-lsp/indexing".to_string());
            self.client
                .send_request::<WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                    token: token.clone(),
                })
                .await
                .ok();

            let (
                exclude_paths,
                include_paths,
                max_indexed_files,
                debug,
                cache_path,
                warm_analysis,
                flush_interval_ms,
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
            if let Some(ref c) = first_root_cache {
                self.docs
                    .set_session_cache_dir(c.cache_dir().join("session"));
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
                warm_docs.current_analysis_session();
            });

            let docs = Arc::clone(&self.docs);
            let open_files = self.open_files.clone();
            let client = self.client.clone();
            let psr4 = self.psr4.clone();
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
                let mut from_cache = 0usize;
                let mut first_root_cache = first_root_cache;
                for root in &roots {
                    // The first root's cache was already opened synchronously
                    // above (to attach the session cache dir before the
                    // prewarm task could race it); reuse it here instead of
                    // opening it twice. Later roots (multi-root workspaces)
                    // still open their own.
                    let cache = first_root_cache.take().or_else(|| {
                        if let Some(ref p) = cache_path {
                            Some(crate::index::cache::WorkspaceCache::with_dir(p.clone()))
                        } else {
                            crate::index::cache::WorkspaceCache::new(root)
                        }
                    });
                    let (n, c) = scan_workspace(
                        root.clone(),
                        Arc::clone(&docs),
                        open_files.clone(),
                        cache,
                        &exclude_paths,
                        &include_paths,
                        max_indexed_files,
                        Some(progress_tx.clone()),
                    )
                    .await;
                    total += n;
                    from_cache += c;
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
                    let parsed = total.saturating_sub(from_cache);
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
                                "php-lsp: debug: {from_cache} from cache, {parsed} parsed fresh \
                                 | {ns_count} PSR-4 namespaces | roots: {root_list}"
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
                // no-ops-per-file on a first-ever run.
                {
                    let pre = Arc::clone(&salsa_docs);
                    let _ = tokio::task::spawn_blocking(move || {
                        pre.get_workspace_index_salsa();
                        pre.warm_start_indexes();
                    })
                    .await;
                }
                docs.mark_index_ready();
                drop(docs);
                client.send_notification::<IndexReadyNotification>(()).await;
                let sweep_open = open_files.urls();
                drop(tokio::task::spawn_blocking(move || {
                    // Warm mir's `analyze_file` memos across the workspace so
                    // the first references/rename on any symbol answers from
                    // memo hits instead of a cold multi-second analysis. Files
                    // the user already has open (and their dependencies) warm
                    // first.
                    if warm_analysis {
                        let cancel = salsa_docs.begin_warm_sweep();
                        salsa_docs.warm_analysis_sweep(&sweep_open, &cancel);
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
        let mut merged_changes: std::collections::HashMap<Url, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file_rename in &params.files {
            let old_path = Url::parse(&file_rename.old_uri)
                .ok()
                .and_then(|u| u.to_file_path().ok());
            let new_path = Url::parse(&file_rename.new_uri)
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

            // `use` lines rewrite the full import path (the namespace may
            // change, not just the class name). Only importers of the old
            // FQN can carry such a line, and the workspace index already
            // records per-file imports — no text scan, parse only matches.
            let old_short = fqn_short_name(&old_fqn).to_string();
            let new_short = fqn_short_name(&new_fqn).to_string();
            if old_fqn != new_fqn {
                for uri in self.docs.files_importing(&old_fqn) {
                    let Some(doc) = self.docs.get_doc_salsa(&uri) else {
                        continue;
                    };
                    let edits = use_edits_in_source(doc.source(), &old_fqn, &new_fqn);
                    if !edits.is_empty() {
                        merged_changes.entry(uri).or_default().extend(edits);
                    }
                }
            }

            // Also rename the declaration itself plus reference sites (type
            // hints, `new`, static calls, hierarchy clauses) from mir's
            // posting lists; `use` lines live under separate `use:` keys, so
            // this can't overlap the edits above.
            if old_short != new_short {
                let symbol =
                    mir_analyzer::Name::class(old_fqn.trim_start_matches('\\').to_string());
                let files: Vec<std::sync::Arc<str>> = self.docs.reference_candidate_files(&symbol);
                let docs = std::sync::Arc::clone(&self.docs);
                let locations = tokio::task::spawn_blocking(move || {
                    let (_interactive, cancel_rev) = docs.settled_write_rev_guard();
                    docs.indexed_references(&symbol, &files, true, Some(cancel_rev))
                })
                .await
                .unwrap_or_default();
                for loc in locations
                    .into_iter()
                    .filter_map(crate::navigation::references::session_tuple_to_location)
                {
                    let Some(doc) = self.docs.get_doc_salsa(&loc.uri) else {
                        continue;
                    };
                    let Some(range) = crate::editing::rename::narrow_range_to_word(
                        doc.source(),
                        loc.range,
                        &old_short,
                    ) else {
                        continue;
                    };
                    merged_changes.entry(loc.uri).or_default().push(TextEdit {
                        range,
                        new_text: new_short.clone(),
                    });
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
        for file_rename in &params.files {
            if let Ok(old_uri) = Url::parse(&file_rename.old_uri) {
                self.docs.remove(&old_uri);
                // Clear diagnostics under the old path — same as did_delete_files —
                // or a client keeps showing them for a URI that no longer exists.
                self.open_files
                    .note_published(&old_uri, crate::backend::diagnostics_content_hash(&[]));
                self.client.publish_diagnostics(old_uri, vec![], None).await;
            }
            if let Ok(new_uri) = Url::parse(&file_rename.new_uri)
                && let Ok(path) = new_uri.to_file_path()
                && let Ok(text) = tokio::fs::read_to_string(&path).await
            {
                self.ingest_if_not_open(new_uri, &text);
            }
        }
        send_refresh_requests(&self.client).await;
    }

    pub(crate) async fn handle_will_create_files(
        &self,
        params: CreateFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        let psr4 = self.psr4.load();
        let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> =
            std::collections::HashMap::new();

        for file in &params.files {
            let Ok(uri) = Url::parse(&file.uri) else {
                continue;
            };
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

    pub(crate) async fn handle_did_create_files(&self, params: CreateFilesParams) {
        for file in &params.files {
            if let Ok(uri) = Url::parse(&file.uri)
                && let Ok(path) = uri.to_file_path()
                && let Ok(text) = tokio::fs::read_to_string(&path).await
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

            // Only importers of the FQN can carry a deletable `use` line;
            // the workspace index records per-file imports — no text scan.
            for uri in self.docs.files_importing(&fqn) {
                let Some(doc) = self.docs.get_doc_salsa(&uri) else {
                    continue;
                };
                let edits = delete_use_in_source(doc.source(), &fqn);
                if !edits.is_empty() {
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

    pub(crate) async fn handle_did_delete_files(&self, params: DeleteFilesParams) {
        for file in &params.files {
            if let Ok(uri) = Url::parse(&file.uri) {
                self.docs.remove(&uri);
                self.open_files
                    .note_published(&uri, crate::backend::diagnostics_content_hash(&[]));
                self.client.publish_diagnostics(uri, vec![], None).await;
            }
        }
        send_refresh_requests(&self.client).await;
    }
}
