#[cfg(not(feature = "dhat-heap"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use tower_lsp_server::Server;

#[cfg(not(feature = "dhat-heap"))]
mod mi {
    use std::ffi::{c_int, c_long};
    unsafe extern "C" {
        pub fn mi_option_set(option: c_int, value: c_long);
        #[cfg(test)]
        pub fn mi_option_get(option: c_int) -> c_long;
    }
    // Ids from mimalloc v3's `mi_option_e`.
    pub const PURGE_DELAY: c_int = 15;
    pub const PAGE_COMMIT_ON_DEMAND: c_int = 40;
}

/// An idle server never reaches mimalloc's delayed purge, so free pages are returned at once.
#[cfg(not(feature = "dhat-heap"))]
fn tune_allocator() {
    use mi::*;
    for (id, env, value) in [
        (PURGE_DELAY, "MIMALLOC_PURGE_DELAY", 0),
        (PAGE_COMMIT_ON_DEMAND, "MIMALLOC_PAGE_COMMIT_ON_DEMAND", 1),
    ] {
        if std::env::var_os(env).is_none() {
            unsafe { mi_option_set(id, value) };
        }
    }
}

fn main() {
    #[cfg(not(feature = "dhat-heap"))]
    tune_allocator();
    // Cap the blocking pool well below tokio's 512 default. mimalloc keeps a
    // heap per thread, and memory freed cross-thread is only recycled when
    // the owning thread allocates again — a burst-grown pool leaves hundreds
    // of idle heaps stranding freed pages, and session RSS climbs ~5 MB per
    // edit (measured by benches/rss_session.rs; the cap cuts it ~6x). All
    // blocking tasks here are bounded (no blocking task waits unboundedly on
    // another), so a small pool only queues work, never deadlocks.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .max_blocking_threads(16)
        // Blocking threads run salsa queries; match the analysis stack size
        // (see document_store::ensure_rayon_worker_stacks — reserved pages,
        // not committed).
        .thread_stack_size(64 * 1024 * 1024)
        .build()
        .expect("tokio runtime");
    rt.block_on(main_async());
    // Dropping the runtime joins every blocking task; a sweep that missed its
    // cancel (e.g. mid-file on a huge AST) must not keep the process alive
    // after `exit`.
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
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

#[cfg(all(test, not(feature = "dhat-heap")))]
mod tests {
    #[test]
    fn mimalloc_option_ids_match_the_linked_version() {
        // purge_delay is the only option defaulting to 1000 ms.
        if std::env::var_os("MIMALLOC_PURGE_DELAY").is_none() {
            assert_eq!(
                unsafe { super::mi::mi_option_get(super::mi::PURGE_DELAY) },
                1000
            );
        }
    }
}
