mod ast;
mod autoload;
mod backend;
mod call_hierarchy;
mod code_lens;
mod completion;
mod declaration;
mod definition;
mod diagnostics;
mod docblock;
mod document_highlight;
mod file_rename;
mod on_type_format;
mod document_link;
mod document_store;
mod folding;
mod formatting;
mod hover;
mod implementation;
mod inlay_hints;
mod extract_action;
mod generate_action;
mod implement_action;
mod inline_value;
mod moniker;
mod phpdoc_action;
mod phpstorm_meta;
mod references;
mod rename;
mod selection_range;
mod semantic_diagnostics;
mod semantic_tokens;
mod signature_help;
mod stubs;
mod symbols;
mod type_definition;
mod type_hierarchy;
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
