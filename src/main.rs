mod autoload;
mod backend;
mod call_hierarchy;
mod completion;
mod definition;
mod diagnostics;
mod docblock;
mod document_highlight;
mod document_store;
mod folding;
mod hover;
mod implementation;
mod inlay_hints;
mod references;
mod rename;
mod selection_range;
mod semantic_diagnostics;
mod semantic_tokens;
mod signature_help;
mod symbols;
mod type_map;
mod use_resolver;
mod util;
mod walk;

use backend::Backend;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
