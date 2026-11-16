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
mod lang; //     PHP-language model: config, autoload, docblock, php_names
mod laravel; //  Laravel framework support: project detection, string-key index (env/config/view/...)
mod process; //  running external programs with a timeout
mod text; //     generic text mechanics: offset, word, fuzzy, range
mod types; //    type resolution & symbol lookup: type_map, type_query, resolve, symbol_map, stub_members

// Native builds expose only the server and the items below, so rustc flags code nothing reaches;
// the wasm32-wasip1 library build is consumed through these modules directly.
macro_rules! feature_modules {
    ($($name:ident),* $(,)?) => {$(
        #[cfg(target_family = "wasm")]
        pub mod $name;
        #[cfg(not(target_family = "wasm"))]
        mod $name;
    )*};
}

// ── LSP feature capabilities ─────────────────────────────────────────────────
feature_modules!(actions, analysis, completion, editing, hover, navigation);

// ── Salsa query layer ────────────────────────────────────────────────────────
feature_modules!(db);

// ── Server entry point & cross-cutting infrastructure ────────────────────────
// The server needs threads, files and child processes. Everything above it builds for
// wasm32-wasip1, so the completion, hover and analysis code can run in a browser worker.
#[cfg(not(target_family = "wasm"))]
pub mod backend;
#[cfg(test)]
mod test_utils;

// Entry points that benchmarks and integration tests drive directly.
pub use analysis::code_lens::{CodeLensScopeCache, code_lenses, resolve_code_lens};
pub use analysis::inlay_hints::inlay_hints;
#[cfg(any(test, feature = "test-hooks"))]
pub use analysis::semantic_diagnostics::semantic_diagnostics;
pub use analysis::semantic_tokens::{semantic_tokens, semantic_tokens_range};
pub use completion::{CompletionCtx, filtered_completions_at};
pub use db::workspace_index::WorkspaceIndexData;
pub use document::ast::ParsedDoc;
pub use document::document_store::{DocumentStore, ensure_rayon_worker_stacks};
#[cfg(any(test, feature = "test-hooks"))]
pub use hover::hover_info_with_maps;
pub use index::file_index::{ClassDef, ClassKind, FileIndex, Visibility};
pub use lang::config::DiagnosticsConfig;
pub use navigation::call_hierarchy::prepare_call_hierarchy_indexed;
pub use navigation::definition::goto_definition;
pub use navigation::symbols::{document_symbols, workspace_symbols_from_workspace};
pub use types::symbol_map::SymbolMap;
