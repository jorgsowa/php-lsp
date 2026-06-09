// Private items in the modules below are only reachable through main.rs/backend.rs,
// not through this lib entry point, so rustc would flag them as dead. Pub items in
// a lib crate are never subject to dead_code, so only genuinely-internal items are
// suppressed — real dead code within public API surfaces will still be caught.
#![allow(dead_code)]

// Public modules exposed for benchmark crates.
pub mod ast;
pub mod cache;
pub mod completion;
pub mod config;
pub mod db;
pub mod docblock;
pub mod document_store;
pub mod hover;
pub mod resolve;
pub mod type_map;
pub mod type_query;
pub mod util;
pub mod walk;

// Public module: compact symbol index for background-indexed files.
pub mod file_index;

// Public module: per-file memoized symbol map (name → Vec<SymbolEntry>).
pub mod symbol_map;

// Feature groups.
pub mod actions;
pub mod analysis;
pub mod editing;
pub mod navigation;

// Infrastructure modules.
mod autoload;
pub mod backend;
mod file_rename;
mod open_files;
pub mod panic_guard;
mod phpstorm_meta;
mod stubs;
pub mod symbols;
#[cfg(test)]
mod test_utils;
mod use_import;
mod workspace_scan;

// Re-exports for benchmark crates that use the flat `php_lsp::X` paths.
pub use analysis::semantic_diagnostics;
pub use editing::rename;
pub use navigation::call_hierarchy;
pub use navigation::definition;
pub use navigation::implementation;
pub use navigation::references;
