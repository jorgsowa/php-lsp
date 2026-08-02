#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use tower_lsp_server::Server;

fn main() {
    // Cap the blocking pool well below tokio's 512 default. mimalloc keeps a
    // heap per thread, and memory freed cross-thread is only recycled when
    // the owning thread allocates again — a burst-grown pool leaves hundreds
    // of idle heaps stranding freed pages, and session RSS climbs ~5 MB per
    // edit (measured by benches/rss_session.rs; the cap cuts it ~6x). All
    // blocking tasks here are bounded (no blocking task waits unboundedly on
    // another), so a small pool only queues work, never deadlocks.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(16)
        // Blocking threads run salsa queries; match the analysis stack size
        // (see document_store::ensure_rayon_worker_stacks — reserved pages,
        // not committed).
        .thread_stack_size(64 * 1024 * 1024)
        .build()
        .expect("tokio runtime")
        .block_on(main_async());
}

async fn main_async() {
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

    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let log_hint = if std::env::var_os("RUST_LOG").is_some() {
        " (tracing → stderr)"
    } else {
        " (set RUST_LOG=php_lsp=debug to enable tracing)"
    };
    eprintln!(
        "php-lsp {} ({build}) — listening on stdin/stdout{log_hint}",
        env!("CARGO_PKG_VERSION")
    );
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = php_lsp::backend::build_lsp_service();
    Server::new(stdin, stdout, socket).serve(service).await;
}
