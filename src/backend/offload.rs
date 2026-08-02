//! Run CPU-bound handler work off the async runtime worker.
//!
//! tower-lsp drives every connection's requests from the same small tokio
//! worker pool. A handler that walks an AST, scans the workspace index, or
//! shells out to an external process inline blocks that worker — every
//! other in-flight request (on this connection or, with a multi-threaded
//! runtime, any other task sharing the pool) stalls until it finishes. See
//! the git history for the long tail of `fix(lsp): run X off the request
//! loop` commits this fixed one handler at a time before this module existed.
//!
//! [`run`] and [`run_gated`] are the two sanctioned ways to leave the async
//! runtime worker: `tokio::task::spawn_blocking` plus uniform panic
//! handling. A `spawn_blocking` panic is delivered as `Err(JoinError)`
//! rather than propagating like a normal panic; before this module several
//! call sites discarded that `Err` via `.unwrap_or_default()` with no
//! logging at all, so a panicking closure failed silently. Both helpers log
//! unconditionally and return `None`, so callers can never reintroduce that
//! gap by accident — `.unwrap_or_default()`, `.unwrap_or(fallback)`, and
//! `let Some(x) = ... else { return .. }` all stay available on the
//! `Option` they return.
//!
//! `run_gated` additionally passes through a named [`DebugGate`](super::debug_gate::DebugGate)
//! section first, so a responsiveness regression test can pin that specific
//! closure in flight and assert the connection still answers other requests
//! meanwhile (see `debug_gate` module docs). Reach for `run` when a closure
//! has no such test.

use std::sync::Arc;

use super::debug_gate::DebugGate;

/// Runs `f` on the blocking thread pool. Returns `None` (logging via
/// `label`) if `f` panics.
pub async fn run<F, R>(label: &'static str, f: F) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!("{label} panicked: {e}");
            None
        }
    }
}

/// Same as [`run`], but `f` first passes through `section` on `gate` — a
/// no-op outside test builds (see `debug_gate`).
pub async fn run_gated<F, R>(gate: &Arc<DebugGate>, section: &'static str, f: F) -> Option<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    let gate = Arc::clone(gate);
    run(section, move || {
        gate.pass(section);
        f()
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_value_when_no_panic() {
        let r = run("test", || 42).await;
        assert_eq!(r, Some(42));
    }

    #[tokio::test]
    async fn returns_none_on_panic() {
        let r: Option<i32> = run("test", || panic!("boom")).await;
        assert_eq!(r, None);
    }

    #[tokio::test]
    async fn gated_returns_value_when_no_panic() {
        let gate = Arc::new(DebugGate::default());
        let r = run_gated(&gate, "sec", || 7).await;
        assert_eq!(r, Some(7));
    }

    #[tokio::test]
    async fn gated_returns_none_on_panic() {
        let gate = Arc::new(DebugGate::default());
        let r: Option<i32> = run_gated(&gate, "sec", || panic!("boom")).await;
        assert_eq!(r, None);
    }
}
