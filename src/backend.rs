use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tower_lsp::jsonrpc::Result;
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
use crate::folding::folding_ranges;
use crate::formatting::{format_document, format_range};
use crate::hover::hover_info;
use crate::implementation::goto_implementation;
use crate::inlay_hints::inlay_hints;
use crate::phpdoc_action::phpdoc_actions;
use crate::references::find_references;
use crate::rename::{prepare_rename, rename};
use crate::selection_range::selection_ranges;
use crate::semantic_diagnostics::semantic_diagnostics;
use crate::semantic_tokens::{legend, semantic_tokens};
use crate::signature_help::signature_help;
use crate::symbols::{document_symbols, workspace_symbols};
use crate::type_definition::goto_type_definition;
use crate::type_hierarchy::{prepare_type_hierarchy, subtypes_of, supertypes_of};
use crate::util::word_at;

pub struct Backend {
    client: Client,
    docs: Arc<DocumentStore>,
    root_path: Arc<RwLock<Option<PathBuf>>>,
    psr4: Arc<RwLock<Psr4Map>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend {
            client,
            docs: Arc::new(DocumentStore::new()),
            root_path: Arc::new(RwLock::new(None)),
            psr4: Arc::new(RwLock::new(Psr4Map::empty())),
        }
    }
}

#[async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Store root path for workspace scanning
        let root = params
            .root_uri
            .as_ref()
            .and_then(|u| u.to_file_path().ok())
            .or_else(|| {
                params
                    .workspace_folders
                    .as_ref()?
                    .first()?
                    .uri
                    .to_file_path()
                    .ok()
            });
        if let Some(path) = root {
            *self.root_path.write().unwrap() = Some(path);
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        "$".to_string(),
                        ">".to_string(),
                        ":".to_string(),
                        "(".to_string(),
                    ]),
                    ..Default::default()
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                inlay_hint_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: legend(),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..Default::default()
                        },
                    ),
                ),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                declaration_provider: Some(DeclarationCapability::Simple(true)),
                type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
                code_lens_provider: Some(CodeLensOptions {
                    resolve_provider: None,
                }),
                document_formatting_provider: Some(OneOf::Left(true)),
                document_range_formatting_provider: Some(OneOf::Left(true)),
                document_link_provider: Some(DocumentLinkOptions {
                    resolve_provider: Some(false),
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
                        workspace_diagnostics: false,
                        work_done_progress_options: Default::default(),
                    },
                )),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
        // Register a file watcher so we hear about PHP files created/changed/deleted on disk
        let registration = Registration {
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
        };
        self.client
            .register_capability(vec![registration])
            .await
            .ok();

        // Load PSR-4 autoload map and kick off background workspace scan
        if let Some(root) = self.root_path.read().unwrap().clone() {
            // Build PSR-4 map synchronously — it's just JSON file reads, very fast
            *self.psr4.write().unwrap() = Psr4Map::load(&root);

            let docs = Arc::clone(&self.docs);
            let client = self.client.clone();
            tokio::spawn(async move {
                let count = scan_workspace(root, docs).await;
                client
                    .log_message(
                        MessageType::INFO,
                        format!("php-lsp: indexed {count} workspace files"),
                    )
                    .await;
            });
        }

        self.client
            .log_message(MessageType::INFO, "php-lsp ready")
            .await;
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
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
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
        tokio::spawn(async move {
            // 100 ms debounce: if another edit arrives before we parse, the
            // version check in apply_parse will discard this stale result.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let (doc, diagnostics) = tokio::task::spawn_blocking(move || parse_document(&text))
                .await
                .unwrap_or_else(|_| (ParsedDoc::default(), vec![]));

            // Only apply if no newer edit arrived while we were parsing
            if docs.apply_parse(&uri, doc, diagnostics.clone(), version) {
                client.publish_diagnostics(uri, diagnostics, None).await;
            }
        });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.close(&uri);
        // Clear editor diagnostics; the file stays indexed for cross-file features
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        for change in params.changes {
            match change.typ {
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    if let Ok(path) = change.uri.to_file_path() {
                        if let Ok(text) = tokio::fs::read_to_string(&path).await {
                            self.docs.index(change.uri, &text);
                        }
                    }
                }
                FileChangeType::DELETED => {
                    self.docs.remove(&change.uri);
                }
                _ => {}
            }
        }
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
        Ok(Some(CompletionResponse::Array(filtered_completions_at(
            &doc,
            &other_docs,
            trigger,
            Some(&source),
            Some(position),
        ))))
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
        if let Some(word) = word_at(&source, position) {
            if word.contains('\\') {
                if let Some(loc) = self.psr4_goto(&word).await {
                    return Ok(Some(GotoDefinitionResponse::Scalar(loc)));
                }
            }
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
        let all_docs = self.docs.all_docs();
        Ok(Some(rename(&word, &params.new_name, &all_docs)))
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
        Ok(hover_info(&source, &doc, position))
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

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let uri = &params.text_document.uri;
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let links = document_links(uri, &doc, doc.source());
        Ok(if links.is_empty() { None } else { Some(links) })
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
                let filter = params
                    .arguments
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let root = self.root_path.read().unwrap().clone();
                let client = self.client.clone();

                tokio::spawn(async move {
                    let output = tokio::process::Command::new("vendor/bin/phpunit")
                        .arg("--filter")
                        .arg(&filter)
                        .current_dir(root.as_deref().unwrap_or(std::path::Path::new(".")))
                        .output()
                        .await;

                    let message = match output {
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

                    client.show_message(MessageType::INFO, message).await;
                });

                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let uri = &params.text_document.uri;

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
        let sem_diags = semantic_diagnostics(uri, &doc, &other_docs);

        let mut items = parse_diags;
        items.extend(sem_diags);

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

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = &params.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();
        let doc = match self.docs.get_doc(uri) {
            Some(d) => d,
            None => return Ok(None),
        };
        let other_docs = self.docs.other_docs(uri);

        // Semantic diagnostics — collect undefined symbols and offer "Add use import"
        let sem_diags = semantic_diagnostics(uri, &doc, &other_docs);

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

        // PHPDoc generation actions
        actions.extend(phpdoc_actions(uri, &doc, &source, params.range));

        Ok(if actions.is_empty() {
            None
        } else {
            Some(actions)
        })
    }
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
                        if let StmtKind::Class(c) = &inner_stmt.kind {
                            if c.name == Some(name) {
                                return Some(match ns_name {
                                    Some(ref ns) => format!("{ns}\\{name}"),
                                    None => name.to_string(),
                                });
                            }
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

/// Maximum number of PHP files indexed during a workspace scan.
/// Prevents excessive memory use on projects with very large vendor trees.
const MAX_INDEXED_FILES: usize = 50_000;

/// Recursively scan `root` for `*.php` files and add them to the document store.
/// Skips hidden directories (names starting with `.`).
/// Vendor packages are now included so cross-file features work on dependencies.
/// Returns the number of files indexed.
async fn scan_workspace(root: PathBuf, docs: Arc<DocumentStore>) -> usize {
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
            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                // Skip hidden directories only; vendor is now indexed
                if !name.starts_with('.') {
                    stack.push(path);
                }
            } else if file_type.is_file() && path.extension().map_or(false, |e| e == "php") {
                if let Ok(uri) = Url::from_file_path(&path) {
                    if let Ok(text) = tokio::fs::read_to_string(&path).await {
                        docs.index(uri, &text);
                        count += 1;
                    }
                }
            }
        }
    }

    count
}
