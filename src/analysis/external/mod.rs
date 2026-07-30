//! External static-analysis tool integration (PHPStan / PHPCS).
//!
//! These run as child processes against a single saved file and their
//! findings are merged into the diagnostics the built-in `mir`-backed
//! analyzer already produces — see [`crate::lang::config::ExternalToolsConfig`]
//! for why both default off.
pub mod phpcs;
pub mod phpstan;

use std::path::Path;

use tower_lsp::lsp_types::Diagnostic;

use crate::lang::config::ExternalToolsConfig;

/// Run every enabled external tool against `path` and return their combined
/// diagnostics. Tools run one after another rather than concurrently so a
/// single caller gets one complete `Vec` back instead of having to merge two
/// independently-arriving results.
pub async fn run_external_diagnostics(
    cfg: &ExternalToolsConfig,
    path: &Path,
    workspace_root: Option<&Path>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if cfg.phpstan.enabled {
        out.extend(phpstan::run(&cfg.phpstan, path, workspace_root).await);
    }
    if cfg.phpcs.enabled {
        out.extend(phpcs::run(&cfg.phpcs, path, workspace_root).await);
    }
    out
}
