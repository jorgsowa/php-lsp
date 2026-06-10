//! Position → resolved-type queries backed by mir's body analysis.
//!
//! mir's `FileAnalyzer::analyze` already resolves a [`mir_analyzer::Type`] for
//! every expression it visits and records it as a [`mir_analyzer::ResolvedSymbol`]
//! (see `DocumentStore::cached_analysis`, which retains the result across LSP
//! requests). This module is the thin glue that maps an LSP cursor onto those
//! recorded symbols — replacing the hand-rolled, short-name-only tracker in
//! `type_map` for the variable/expression-type cases.
//!
//! ## Contract callers must respect
//!
//! mir symbol spans are **end-exclusive and identifier-only**: the variable
//! `$q` at bytes `76..78` is found by `symbol_at(76)` or `symbol_at(77)` but
//! **not** `symbol_at(78)`. Callers must pass a byte offset that lands strictly
//! inside the token of interest — for a variable, `word_range_at(..).start`
//! (the `$`) is always inside. The primitive is intentionally dumb: it does no
//! offset fudging, because only the caller has the AST context to pick a
//! correct in-token offset without grabbing an adjacent token.

use mir_analyzer::{FileAnalysis, Type};
use mir_types::Atomic;

/// The resolved mir type recorded at `offset`, or `None` if no recorded symbol
/// covers it. `offset` is a byte offset that must land strictly inside the
/// token of interest (see the module-level contract).
pub(crate) fn type_at_offset(analysis: &FileAnalysis, offset: u32) -> Option<&Type> {
    analysis.symbol_at(offset).map(|s| &s.resolved_type)
}

/// The class/enum FQCN named by a single atomic, if any. Covers object types
/// (`TNamedObject`, `self`/`static`/`parent`) and enum-case literals
/// (`Suit::Hearts` → `Suit`), which is what member/declaration lookups want.
fn atomic_class_fqcn(atomic: &Atomic) -> Option<&str> {
    atomic.named_object_fqcn().or(match atomic {
        Atomic::TLiteralEnumCase { enum_fqcn, .. } => Some(enum_fqcn.as_ref()),
        _ => None,
    })
}

/// The fully-qualified class names named by `ty` — one per named-object atomic,
/// so a union `A|B` yields `["A", "B"]`. Generic parameters are stripped
/// (`Collection<User>` → `Collection`), since callers searching for a class
/// declaration match on the bare FQCN. Scalar/array/callable types yield an
/// empty vec.
///
/// Names are returned exactly as mir produces them: fully qualified with no
/// leading `\` (e.g. `App\Svc\User`), already resolved through the file's
/// namespace and `use` imports.
///
/// **TParent caveat**: `Atomic::TParent { fqcn }` carries the *containing*
/// class's FQCN (e.g. `"ChildClass"`), not the actual parent. This function
/// returns that fqcn as-is, which is correct for `self`/`static` navigation
/// but wrong for `parent`. type_definition.rs works around this by bypassing
/// mir for `parent`-typed parameters. The proper fix is a mir API
/// `AnalysisSession::resolve_parent_fqcn(fqcn) → Option<String>` that looks
/// up the extends chain; once available, callers can substitute it here.
pub(crate) fn class_names(ty: &Type) -> Vec<String> {
    ty.types
        .iter()
        .filter_map(atomic_class_fqcn)
        .map(str::to_owned)
        .collect()
}

/// The single receiver class FQCN for member resolution — the first named
/// object in `ty`. `None` if `ty` names no class.
pub(crate) fn primary_class_name(ty: &Type) -> Option<String> {
    ty.types
        .iter()
        .find_map(atomic_class_fqcn)
        .map(str::to_owned)
}
