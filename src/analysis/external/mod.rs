//! External static-analysis tool integration (PHPStan / PHPCS).
//!
//! These run as child processes against a single saved file and their
//! findings are merged into the diagnostics the built-in `mir`-backed
//! analyzer already produces — see [`crate::lang::config::ExternalToolsConfig`]
//! for why both default off.
#[cfg(not(target_family = "wasm"))]
pub mod phpcs;
#[cfg(not(target_family = "wasm"))]
pub mod phpstan;

#[cfg(not(target_family = "wasm"))]
use std::path::Path;

#[cfg(not(target_family = "wasm"))]
use tower_lsp_server::ls_types::Diagnostic;

#[cfg(not(target_family = "wasm"))]
use crate::lang::config::ExternalToolsConfig;

#[cfg(not(target_family = "wasm"))]
const MAX_CONCURRENT_RUNS: usize = 2;

#[cfg(not(target_family = "wasm"))]
static RUN_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_CONCURRENT_RUNS);

/// Wait for one of the [`MAX_CONCURRENT_RUNS`] slots; hold it across
/// [`run_external_diagnostics`] so rapid saves queue instead of piling up
/// processes.
#[cfg(not(target_family = "wasm"))]
pub async fn acquire_run_slot() -> tokio::sync::SemaphorePermit<'static> {
    RUN_SLOTS
        .acquire()
        .await
        .expect("RUN_SLOTS is never closed")
}

/// One in-flight run per file: a newer save, an edit or a close aborts the
/// previous run, which kills its process and frees its slot.
#[cfg(not(target_family = "wasm"))]
#[derive(Default)]
pub struct RunRegistry {
    runs: dashmap::DashMap<tower_lsp_server::ls_types::Uri, tokio::task::AbortHandle>,
}

#[cfg(not(target_family = "wasm"))]
impl RunRegistry {
    /// Spawn `run` for `uri`, aborting the run it supersedes.
    pub fn start(
        &self,
        uri: tower_lsp_server::ls_types::Uri,
        run: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let handle = tokio::spawn(run).abort_handle();
        if let Some(previous) = self.runs.insert(uri, handle) {
            previous.abort();
        }
    }

    pub fn cancel(&self, uri: &tower_lsp_server::ls_types::Uri) {
        if let Some((_, run)) = self.runs.remove(uri) {
            run.abort();
        }
    }
}

/// SIGKILLs a tool's process group on drop, so processes it forked die with it.
#[cfg(unix)]
struct ProcessGroupGuard(Option<u32>);

#[cfg(unix)]
impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.0.and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: `killpg` has no memory-safety preconditions.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

/// Run `command` to completion, killing it and its children if it outlives
/// `timeout` or the run is aborted.
#[cfg(not(target_family = "wasm"))]
async fn output_with_timeout(
    mut command: tokio::process::Command,
    timeout: std::time::Duration,
    tool: &str,
) -> Option<std::process::Output> {
    command.kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::debug!("{tool} did not run: {e}");
            return None;
        }
    };
    #[cfg(unix)]
    let _group = ProcessGroupGuard(child.id());
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Some(output),
        Ok(Err(e)) => {
            tracing::debug!("{tool} did not run: {e}");
            None
        }
        Err(_) => {
            tracing::warn!("{tool} killed after {timeout:?}");
            None
        }
    }
}

/// Run every enabled external tool against `path` and return their combined
/// diagnostics. Tools run one after another rather than concurrently so a
/// single caller gets one complete `Vec` back instead of having to merge two
/// independently-arriving results.
#[cfg(not(target_family = "wasm"))]
pub async fn run_external_diagnostics(
    cfg: &ExternalToolsConfig,
    path: &Path,
    workspace_root: Option<&Path>,
) -> Vec<Diagnostic> {
    let timeout = std::time::Duration::from_millis(cfg.timeout_ms);
    let mut out = Vec::new();
    if cfg.phpstan.enabled {
        out.extend(phpstan::run(&cfg.phpstan, path, workspace_root, timeout).await);
    }
    if cfg.phpcs.enabled {
        out.extend(phpcs::run(&cfg.phpcs, path, workspace_root, timeout).await);
    }
    out
}
