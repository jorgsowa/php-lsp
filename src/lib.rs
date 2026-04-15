// Private items in the modules below are only reachable through main.rs/backend.rs,
// not through this lib entry point, so rustc would flag them as dead. Pub items in
// a lib crate are never subject to dead_code, so only genuinely-internal items are
// suppressed — real dead code within public API surfaces will still be caught.
#![allow(dead_code)]

// Public modules exposed for benchmark crates.
pub mod ast;
pub mod completion;
pub mod definition;
pub mod docblock;
pub mod document_store;
pub mod hover;
pub mod type_map;
pub mod util;
pub mod walk;

// Private modules needed transitively by the public ones.
mod autoload;
mod backend;
mod call_hierarchy;
mod code_lens;
mod declaration;
mod diagnostics;
mod document_highlight;
mod document_link;
mod extract_action;
mod extract_constant_action;
mod extract_method_action;
mod file_rename;
mod folding;
mod formatting;
mod generate_action;
mod implement_action;
mod implementation;
mod inlay_hints;
mod inline_action;
mod inline_value;
mod moniker;
mod on_type_format;
mod organize_imports;
mod phpdoc_action;
mod phpstorm_meta;
mod promote_action;
mod references;
mod rename;
mod selection_range;
mod semantic_diagnostics;
mod semantic_tokens;
mod signature_help;
mod stubs;
mod symbols;
#[cfg(test)]
mod test_utils;
mod type_action;
mod type_definition;
mod type_hierarchy;
mod use_import;
