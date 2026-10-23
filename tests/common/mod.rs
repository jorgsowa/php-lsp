//! Shared test harness: a real LSP server wired over in-memory duplex
//! streams, plus a fluent `TestServer` builder to cut down on JSON-RPC
//! boilerplate in tests.
//!
//! The harness speaks the full LSP wire protocol — no internal API shortcuts —
//! so tests exercise the same path a real editor client would.

#![allow(dead_code, unused_imports)]

mod client;
pub mod fixture;
pub mod php_syntax;
pub mod render;
mod server;

pub use client::TestClient;
pub(crate) use client::isolate_cache_home;
pub use render::{
    canonicalize_workspace_edit, lines_of, render_call_hierarchy, render_completion,
    render_completion_ordered, render_diagnostics_notification, render_document_highlight,
    render_document_symbols, render_hover, render_inlay_hints, render_locations,
    render_pull_diagnostics, render_resolved_code_action, render_resolved_code_lens,
    render_resolved_completion_item, render_resolved_document_link, render_resolved_inlay_hint,
    render_resolved_workspace_symbol, render_semantic_tokens, render_semantic_tokens_delta,
    render_signature_help, render_text_edits, render_workspace_diagnostic,
    render_workspace_symbols,
};
pub use server::{OpenedFixture, TestServer};
