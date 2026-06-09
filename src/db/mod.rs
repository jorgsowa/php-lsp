//! Salsa-based incremental computation layer.
//!
//! After the mir-analyzer 0.22 migration, semantic analysis (Pass 2, class
//! issues, references) is owned by `AnalysisSession` in `DocumentStore`.
//! Salsa here is now responsible for the LSP-side compact representations
//! only: parsed AST cache (`parse`), per-file declaration index (`index`),
//! workspace aggregation (`workspace_index`). These power cross-file LSP
//! features (workspace symbols, document symbols, find-implementations, hover
//! from index) that don't require the analyzer's full type system.

pub mod analysis;
pub mod index;
pub mod input;
pub mod parse;
pub mod symbol_map;
pub mod workspace_index;

#[cfg(test)]
mod gc_gate_test;

#[allow(unused_imports)]
pub use input::{FileText, SourceFile, Workspace, workspace_files};

/// Implement the `salsa::Update` trait for Arc-wrapped types using pointer equality.
/// This reduces boilerplate for types that wrap a single Arc field and should only
/// invalidate when the pointer changes (not the contents).
#[macro_export]
macro_rules! impl_arc_update {
    ($ty:ty) => {
        unsafe impl salsa::Update for $ty {
            unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
                let old_ref = unsafe { &mut *old_pointer };
                if std::sync::Arc::ptr_eq(&old_ref.0, &new_value.0) {
                    false
                } else {
                    *old_ref = new_value;
                    true
                }
            }
        }
    };
}
