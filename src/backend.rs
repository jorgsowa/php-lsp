use std::sync::Arc;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{async_trait, Client, LanguageServer};

use crate::completion::filtered_completions;
use crate::definition::goto_definition;
use crate::document_store::DocumentStore;
use crate::hover::hover_info;
use crate::symbols::document_symbols;

pub struct Backend {
    client: Client,
    docs: Arc<DocumentStore>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Backend {
            client,
            docs: Arc::new(DocumentStore::new()),
        }
    }
}

#[async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> Result<InitializeResult> {
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
                document_symbol_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _params: InitializedParams) {
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
        self.client.publish_diagnostics(uri, vec![], None).await;
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
}
