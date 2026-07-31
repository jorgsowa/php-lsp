#![allow(dead_code, unused_imports)]

#[path = "common/mod.rs"]
mod common;

use common::render::render_code_lens;
pub use common::render::{
    assert_linked_editing_ranges_share_text, assert_selection_range_invariant,
};
pub use common::{
    TestServer, lines_of, render_completion, render_diagnostics_notification,
    render_document_symbols, render_hover, render_inlay_hints, render_locations,
    render_pull_diagnostics, render_resolved_inlay_hint, render_semantic_tokens,
    render_semantic_tokens_delta, render_workspace_diagnostic, render_workspace_symbols,
};

#[path = "analysis/feature_code_lens.rs"]
mod feature_code_lens;
#[path = "analysis/feature_diagnostics_edge_cases.rs"]
mod feature_diagnostics_edge_cases;
#[path = "analysis/feature_diagnostics_inheritance.rs"]
mod feature_diagnostics_inheritance;
#[path = "analysis/feature_diagnostics_type_errors.rs"]
mod feature_diagnostics_type_errors;
#[path = "analysis/feature_diagnostics_undefined.rs"]
mod feature_diagnostics_undefined;
#[path = "analysis/feature_diagnostics_workspace.rs"]
mod feature_diagnostics_workspace;
#[path = "analysis/feature_inlay_hints.rs"]
mod feature_inlay_hints;
#[path = "analysis/feature_inline_value.rs"]
mod feature_inline_value;
#[path = "analysis/feature_laravel_eloquent_guard.rs"]
mod feature_laravel_eloquent_guard;
#[path = "analysis/feature_php_versions.rs"]
mod feature_php_versions;
#[path = "analysis/feature_semantic_tokens.rs"]
mod feature_semantic_tokens;
