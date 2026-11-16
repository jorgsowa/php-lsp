//! `symbol_map` salsa query — derives a [`SymbolMap`] from a parsed document.
//!
//! Depends on `parsed_doc`, so editing a file reparses once and then the symbol
//! map rebuilds. Between edits all lookups are served from the cache in O(1).

use std::sync::Arc;

use crate::types::symbol_map::SymbolMap;

/// Arc wrapper for [`SymbolMap`]. `symbol_map` is `#[salsa::tracked(no_eq)]`:
/// every `build` call produces a new `Arc`, so a changed parse always propagates.
#[derive(Clone)]
pub struct SymbolMapArc(pub Arc<SymbolMap>);
