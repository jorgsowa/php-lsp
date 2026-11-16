//! Document lifecycle: the arena-backed parsed document, the salsa-input
//! document store, and the open-file (editor buffer) state.

pub mod ast;
pub mod document_store;

pub(crate) mod cache_registry;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod open_files;
