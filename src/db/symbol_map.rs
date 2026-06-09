//! `symbol_map` salsa query — derives a [`SymbolMap`] from a parsed document.
//!
//! Depends on `parsed_doc`, so editing a file reparses once and then the symbol
//! map rebuilds. Between edits all lookups are served from the cache in O(1).

use std::sync::Arc;

use salsa::Database;

use crate::db::input::SourceFile;
use crate::db::parse::parsed_doc;
use crate::symbol_map::SymbolMap;

/// Arc wrapper for [`SymbolMap`]. Pointer equality drives salsa invalidation:
/// every `build` call produces a new `Arc`, so a changed parse always propagates.
#[derive(Clone)]
pub struct SymbolMapArc(pub Arc<SymbolMap>);

impl SymbolMapArc {
    pub fn get(&self) -> &SymbolMap {
        &self.0
    }
}

crate::impl_arc_update!(SymbolMapArc);

/// Build the symbol map for a file. `no_eq` because `SymbolMapArc` has no
/// structural equality — invalidation flows from `parsed_doc`.
#[salsa::tracked(no_eq, lru = 2048)]
pub fn symbol_map(db: &dyn Database, file: SourceFile<'_>) -> SymbolMapArc {
    let doc = parsed_doc(db, file);
    SymbolMapArc(Arc::new(SymbolMap::build(doc.get())))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use salsa::Setter;

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileText, Workspace, workspace_files};

    static MEMO_CALLS: AtomicUsize = AtomicUsize::new(0);
    static INVAL_CALLS: AtomicUsize = AtomicUsize::new(0);

    #[salsa::tracked]
    fn counted_memo(db: &dyn Database, file: SourceFile<'_>) -> usize {
        MEMO_CALLS.fetch_add(1, Ordering::SeqCst);
        symbol_map(db, file)
            .get()
            .lookup("greet", |_| true)
            .is_some() as usize
    }

    #[salsa::tracked]
    fn counted_inval(db: &dyn Database, file: SourceFile<'_>) -> usize {
        INVAL_CALLS.fetch_add(1, Ordering::SeqCst);
        symbol_map(db, file)
            .get()
            .lookup("greet", |_| true)
            .is_some() as usize
    }

    fn make_ws(host: &AnalysisHost, uri: &str, ft: FileText) -> Workspace {
        Workspace::new(
            host.db(),
            std::sync::Arc::from([(Arc::<str>::from(uri), ft)]),
            mir_analyzer::PhpVersion::LATEST,
        )
    }

    #[test]
    fn symbol_map_builds_and_memoizes() {
        MEMO_CALLS.store(0, Ordering::SeqCst);
        let host = AnalysisHost::new();
        let ft = FileText::new(
            host.db(),
            Arc::<str>::from("<?php\nfunction greet(): void {}"),
            None,
        );
        let ws = make_ws(&host, "file:///memo.php", ft);
        let files = workspace_files(host.db(), ws);
        let _ = counted_memo(host.db(), files[0]);
        let _ = counted_memo(host.db(), files[0]);
        assert_eq!(
            MEMO_CALLS.load(Ordering::SeqCst),
            1,
            "salsa should memoize the second call with unchanged input"
        );
    }

    #[test]
    fn symbol_map_invalidates_on_edit() {
        INVAL_CALLS.store(0, Ordering::SeqCst);
        let mut host = AnalysisHost::new();
        let ft = FileText::new(
            host.db(),
            Arc::<str>::from("<?php\nfunction greet(): void {}"),
            None,
        );
        let ws = make_ws(&host, "file:///inval.php", ft);
        {
            let files = workspace_files(host.db(), ws);
            let _ = counted_inval(host.db(), files[0]);
            assert_eq!(INVAL_CALLS.load(Ordering::SeqCst), 1);
        }

        ft.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\nfunction farewell(): void {}"));
        {
            let files = workspace_files(host.db(), ws);
            let _ = counted_inval(host.db(), files[0]);
            assert_eq!(
                INVAL_CALLS.load(Ordering::SeqCst),
                2,
                "symbol_map should re-run after source text change"
            );
        }
    }
}
