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

// Re-exports so that existing `crate::X` paths in backend.rs, bench crates, and
// cross-module references within moved files all continue to resolve unchanged.
pub use actions::extract_action;
pub use actions::extract_constant_action;
pub use actions::extract_method_action;
pub use actions::generate_action;
pub use actions::implement_action;
pub use actions::inline_action;
pub use actions::phpdoc_action;
pub use actions::promote_action;
pub use actions::type_action;

pub use navigation::call_hierarchy;
pub use navigation::declaration;
pub use navigation::definition;
pub use navigation::implementation;
pub use navigation::moniker;
pub use navigation::references;
pub use navigation::type_definition;
pub use navigation::type_hierarchy;

pub use analysis::code_lens;
pub use analysis::diagnostics;
pub use analysis::document_highlight;
pub use analysis::inlay_hints;
pub use analysis::inline_value;
pub use analysis::semantic_diagnostics;
pub use analysis::semantic_tokens;

pub use editing::document_link;
pub use editing::folding;
pub use editing::formatting;
pub use editing::on_type_format;
pub use editing::organize_imports;
pub use editing::rename;
pub use editing::selection_range;
pub use editing::signature_help;
