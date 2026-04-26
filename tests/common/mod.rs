//! Shared test harness: a real LSP server wired over in-memory duplex
//! streams, plus a fluent `TestServer` builder to cut down on JSON-RPC
//! boilerplate in tests.
//!
//! The harness speaks the full LSP wire protocol — no internal API shortcuts —
//! so tests exercise the same path a real editor client would.

#![allow(dead_code, unused_imports)]

mod client;
pub mod fixture;
mod render;
mod server;

pub use client::TestClient;
pub use render::{
    canonicalize_workspace_edit, render_document_symbols, render_hover, render_workspace_symbols,
};
pub use server::{OpenedFixture, TestServer};
