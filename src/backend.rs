use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{async_trait, Client, LanguageServer};

use crate::completion::filtered_completions;
use crate::definition::goto_definition;
use crate::document_store::DocumentStore;
use crate::hover::hover_info;
use crate::references::find_references;
use crate::rename::{prepare_rename, rename};
use crate::signature_help::signature_help;
use crate::symbols::{document_symbols, workspace_symbols};
use crate::util::word_at;

pub struct Backend {
    client: Client,
    docs: Arc<DocumentStore>,
    root_path: Arc<RwLock<Option<PathBuf>>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend {
            client,
            docs: Arc::new(DocumentStore::new()),
            root_path: Arc::new(RwLock::new(None)),
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
                    trigger_characters: Some(vec!["$".to_string(), ">".to_string()]),
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
        self.client.register_capability(vec![registration]).await.ok();

        // Kick off background workspace scan
        if let Some(root) = self.root_path.read().unwrap().clone() {
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
        self.docs.open(uri.clone(), params.text_document.text);
        let diagnostics = self.docs.get_diagnostics(&uri).unwrap_or_default();
        self.client.publish_diagnostics(uri, diagnostics, None).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        if let Some(change) = params.content_changes.into_iter().last() {
            self.docs.update(uri.clone(), change.text);
            let diagnostics = self.docs.get_diagnostics(&uri).unwrap_or_default();
            self.client.publish_diagnostics(uri, diagnostics, None).await;
        }
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
        let ast = self.docs.get_ast(uri).unwrap_or_default();
        let other_asts = self.docs.other_asts(uri);
        let trigger = params
            .context
            .as_ref()
            .and_then(|c| c.trigger_character.as_deref());
        Ok(Some(CompletionResponse::Array(filtered_completions(
            &ast, &other_asts, trigger,
        ))))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let ast = self.docs.get_ast(uri).unwrap_or_default();
        let other_docs = self.docs.other_docs(uri);
        Ok(goto_definition(uri, &source, &ast, &other_docs, position)
            .map(GotoDefinitionResponse::Scalar))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let word = match word_at(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        let all_docs = self.docs.all_docs_ast();
        let include_declaration = params.context.include_declaration;
        let locations = find_references(&word, &all_docs, include_declaration);
        Ok(if locations.is_empty() { None } else { Some(locations) })
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = &params.text_document.uri;
        let source = self.docs.get(uri).unwrap_or_default();
        Ok(prepare_rename(&source, params.position)
            .map(PrepareRenameResponse::Range))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let word = match word_at(&source, position) {
            Some(w) => w,
            None => return Ok(None),
        };
        let all_docs = self.docs.all_docs_ast();
        Ok(Some(rename(&word, &params.new_name, &all_docs)))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let ast = self.docs.get_ast(uri).unwrap_or_default();
        Ok(signature_help(&source, &ast, position))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let source = self.docs.get(uri).unwrap_or_default();
        let ast = self.docs.get_ast(uri).unwrap_or_default();
        Ok(hover_info(&source, &ast, position))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let ast = self.docs.get_ast(uri).unwrap_or_default();
        Ok(Some(DocumentSymbolResponse::Nested(document_symbols(&ast))))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let docs = self.docs.all_docs_ast();
        let results = workspace_symbols(&params.query, &docs);
        Ok(if results.is_empty() { None } else { Some(results) })
    }
}

/// Recursively scan `root` for `*.php` files and add them to the document store.
/// Skips `vendor/` and hidden directories.
/// Returns the number of files indexed.
async fn scan_workspace(root: PathBuf, docs: Arc<DocumentStore>) -> usize {
    let mut count = 0usize;
    let mut stack = vec![root];

    while let Some(dir) = stack.pop() {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let file_type = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("");
                if !name.starts_with('.') && name != "vendor" {
                    stack.push(path);
                }
            } else if file_type.is_file()
                && path.extension().map_or(false, |e| e == "php")
            {
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
