// Matches the crate-level suppression on `lib.rs`: private items only reached
// through other modules look dead from either entry point.
#![allow(dead_code)]

mod ast;
mod autoload;
mod backend;
mod cache;
mod call_hierarchy;
mod code_lens;
mod completion;
mod db;
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
mod file_index;
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
mod promote_action;
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
mod use_import;
mod util;
mod walk;

#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use backend::Backend;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    // Emit JSON spans to stderr when RUST_LOG is set.
    // Example: RUST_LOG=php_lsp=debug php-lsp 2>trace.jsonl
    // Each closed span includes "time.busy" and "time.idle" duration fields.
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .with_writer(std::io::stderr)
        .init();
    if let Some(arg) = std::env::args().nth(1)
        && (arg == "--version" || arg == "-V")
    {
        println!("php-lsp {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    eprintln!(
        "php-lsp {} — listening on stdin/stdout",
        env!("CARGO_PKG_VERSION")
    );
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
