#![allow(dead_code, unused_imports)]

#[path = "common/mod.rs"]
mod common;

pub use common::render::{
    assert_linked_editing_ranges_share_text, assert_selection_range_invariant,
};
pub use common::{
    TestServer, canonicalize_workspace_edit, lines_of, render_completion,
    render_diagnostics_notification, render_document_symbols, render_hover, render_inlay_hints,
    render_locations, render_pull_diagnostics, render_semantic_tokens, render_workspace_symbols,
};

#[path = "workspace/feature_cache_warm_start.rs"]
mod feature_cache_warm_start;
#[path = "workspace/feature_configuration.rs"]
mod feature_configuration;
#[path = "workspace/feature_doc_lifecycle.rs"]
mod feature_doc_lifecycle;
#[path = "workspace/feature_execute_command.rs"]
mod feature_execute_command;
#[path = "workspace/feature_file_ops.rs"]
mod feature_file_ops;
#[path = "workspace/feature_incremental.rs"]
mod feature_incremental;
#[path = "workspace/feature_indexing_perf.rs"]
mod feature_indexing_perf;
#[path = "workspace/feature_lsp_gaps_verification.rs"]
mod feature_lsp_gaps_verification;
#[path = "workspace/feature_project_structures.rs"]
mod feature_project_structures;
#[path = "workspace/feature_pull_diagnostics.rs"]
mod feature_pull_diagnostics;
#[path = "workspace/feature_push_diagnostics.rs"]
mod feature_push_diagnostics;
#[path = "workspace/feature_server.rs"]
mod feature_server;
#[path = "workspace/feature_uri_edge_cases.rs"]
mod feature_uri_edge_cases;
#[path = "workspace/feature_use_statement_navigation.rs"]
mod feature_use_statement_navigation;
#[path = "workspace/feature_workspace_folders.rs"]
mod feature_workspace_folders;
#[path = "workspace/feature_workspace_scan.rs"]
mod feature_workspace_scan;
