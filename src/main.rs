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
mod document_link;
mod document_store;
mod extract_action;
mod extract_constant_action;
mod extract_method_action;
mod file_rename;
mod folding;
mod formatting;
mod generate_action;
mod hover;
mod implement_action;
mod implementation;
mod inlay_hints;
mod inline_action;
mod inline_value;
mod moniker;
mod on_type_format;
mod organize_imports;
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
#[cfg(test)]
mod test_utils;
mod type_action;
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
    if let Some(arg) = std::env::args().nth(1)
        && (arg == "--version" || arg == "-V")
    {
        println!("php-lsp {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
