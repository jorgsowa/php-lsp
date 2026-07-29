//! # php-lsp — domain vocabulary
//!
//! The terms below have one canonical meaning across the crate. Use them
//! consistently; do not introduce synonyms.
//!
//! ## Core nouns
//! - **`ParsedDoc`** — a fully parsed source file: AST program + source text +
//!   line table, backed by a bumpalo arena. The unit a user has open.
//! - **`FileIndex`** — the compact (~2 KB) serializable per-file *declaration
//!   summary* used for background-indexed (unopened) files.
//! - **`WorkspaceIndex`** (`WorkspaceIndexData`) — the aggregated cross-file
//!   index over every `FileIndex`, with name→location reverse maps.
//! - **`Workspace`** — the salsa input representing the project root / file set.
//!   (There is intentionally no "Codebase" type — it would be a synonym.)
//! - **`SymbolMap` / `SymbolEntry`** — per-file precomputed name→declarations
//!   table for *open* files; richer than `FileIndex`.
//! - **`Declaration`** (`resolve::Declaration`) — the AST node where a symbol is
//!   *introduced*. Shared by both the `goto_definition` and `goto_declaration`
//!   LSP requests (which stay distinct — see `navigation/`).
//! - **`Docblock`** — a PHPDoc `/** … */` comment and the data parsed from it.
//!
//! ## Naming conventions
//! - **verb `index` → `ingest`**: ingesting a file into the store is `ingest*`;
//!   the noun `index` (`FileIndex`, the `file_index` query) is always a data
//!   structure, never an action.
//! - **`doc`** as a value/variable means a `ParsedDoc`. Docblock-derived data
//!   uses the **`Doc`** type prefix (`DocMethod`, `DocProperty`) or a field
//!   named `docblock`. Never name a docblock field `doc`.
//! - **Type suffixes**: `-Def` = an owned declaration record stored in an index
//!   (`FunctionDef`, `ClassDef`); `-Entry` = a row returned from a name-keyed
//!   map (`SymbolEntry`); `-Ref` = an internal back-pointer/handle into an index
//!   (`ClassRef`, `DeclRef`) — **not** an LSP reference. The spelled-out word
//!   `Reference`/`refs` is reserved for a symbol *usage* in code (see
//!   `navigation/references.rs`, `walk.rs`).
//! - **`sv`** is the established local idiom for a borrowed `SourceView`.

// ── Core concern modules ────────────────────────────────────────────────────
mod document; // document lifecycle: parsed AST, salsa store, open-file state
mod index; //    workspace indexing: compact FileIndex, background scan, cache
mod lang; //     PHP-language model: config, autoload, phpstorm_meta, docblock, php_names
mod laravel; //  Laravel framework support: project detection, string-key index (env/config/view/...)
mod text; //     generic text mechanics: offset, word, fuzzy, range
mod types; //    type resolution & symbol lookup: type_map, type_query, resolve, symbol_map, stub_members

// ── LSP feature capabilities ─────────────────────────────────────────────────
pub mod actions;
pub mod analysis;
pub mod completion;
pub mod editing;
pub mod hover;
pub mod navigation;

// ── Salsa query layer ────────────────────────────────────────────────────────
pub mod db;

// ── Server entry point & cross-cutting infrastructure ────────────────────────
pub mod backend;
#[cfg(test)]
mod test_utils;

// Re-exports for benchmark crates and integration tests that use the flat `php_lsp::X` paths.
pub use analysis::semantic_diagnostics;
pub use document::ast;
pub use document::document_store;
pub use index::cache;
pub use index::file_index;
pub use lang::config;
pub use navigation::call_hierarchy;
pub use navigation::definition;
pub use navigation::symbols;
pub use types::symbol_map;
