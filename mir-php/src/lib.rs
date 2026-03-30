//! `mir-php` — PHP static analysis engine.
//!
//! Provides type inference and semantic diagnostics for PHP source code,
//! operating directly on [`php_ast`] AST slices with no dependency on any
//! LSP framework.
//!
//! # Usage
//!
//! ```ignore
//! use bumpalo::Bump;
//! use php_rs_parser::parse;
//!
//! let arena = Bump::new();
//! let program = parse(source, &arena).unwrap();
//!
//! let diags = mir_php::analyze(source, &program.stmts, &[]);
//! let env   = mir_php::infer(&program.stmts);
//! ```

pub mod diag;
pub mod infer;
pub mod stubs;
pub mod types;
pub mod util;

pub use diag::{Diagnostic, Severity};
pub use types::{Ty, TypeEnv};

/// Analyse `stmts` against `all` workspace documents and return diagnostics.
///
/// - `source` — the raw PHP source text for `stmts` (used for position mapping)
/// - `stmts`  — AST of the document being checked
/// - `all`    — every document in the workspace as `(source, stmts)` pairs;
///              include the current document so its definitions are visible
pub fn analyze<'a>(
    source: &str,
    stmts: &[php_ast::Stmt<'a, 'a>],
    all: &[(&str, &[php_ast::Stmt<'a, 'a>])],
) -> Vec<Diagnostic> {
    diag::analyze(source, stmts, all)
}

/// Build a `TypeEnv` (variable → type map) from `stmts`.
pub fn infer<'a>(stmts: &[php_ast::Stmt<'a, 'a>]) -> TypeEnv {
    infer::infer(stmts)
}
