//! Salsa-based incremental computation layer.
//!
//! Semantic analysis (Pass 2, class issues, references) is owned by
//! `AnalysisSession` in `DocumentStore`. Salsa here is responsible for the
//! LSP-side compact representations only: parsed AST cache (`parse`), per-file
//! declaration index (`index`), workspace aggregation (`workspace_index`).
//! These power cross-file LSP features (workspace symbols, document symbols,
//! find-implementations, hover from index) that don't require the analyzer's
//! full type system.

pub mod index;
pub mod mir_queries;
pub mod parse;
pub mod symbol_map;
pub mod workspace_index;

#[cfg(test)]
mod gc_gate_test;

#[cfg(test)]
mod convergence_spike;
