//! Salsa-based incremental computation layer.
//!
//! Phase A scaffold: defines the `RootDatabase`, a `SourceFile` input, and a
//! trivial `parsed_doc` query that wraps `diagnostics::parse_document`. Not yet
//! wired into `Backend` — this exists so downstream phases can grow queries on
//! top of it incrementally.

pub mod analysis;
pub mod codebase;
pub mod definitions;
pub mod index;
pub mod input;
pub mod method_returns;
pub mod parse;
pub mod refs;

#[allow(unused_imports)] // Analysis/RootDatabase reserved for Phase E.
pub use analysis::{Analysis, AnalysisHost, RootDatabase};
#[allow(unused_imports)] // FileId construction is test-only today.
pub use input::{FileId, SourceFile, Workspace};
