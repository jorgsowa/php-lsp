#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use php_lsp::backend::Backend;
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
