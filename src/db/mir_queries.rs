//! php-lsp salsa queries on mir's shared database: keyed on mir's `SourceFile`
//! input and run over `&dyn MirDatabase`, so they live in the single converged
//! db. The Arc-wrapper types they return (`ParsedArc`, `IndexArc`,
//! `SymbolMapArc`) live in `parse.rs` / `index.rs` / `symbol_map.rs`.

use std::sync::Arc;

use mir_analyzer::db::{MirDatabase, SourceFile};
use tower_lsp::lsp_types::Url;

use crate::db::index::IndexArc;
use crate::db::parse::ParsedArc;
use crate::db::symbol_map::SymbolMapArc;
use crate::db::workspace_index::{WorkspaceIndexArc, WorkspaceIndexData};
use crate::document::ast::ParsedDoc;
use crate::index::file_index::FileIndex;
use crate::types::symbol_map::SymbolMap;

/// Per-file php-lsp input on the converged db: pairs mir's shared `SourceFile`
/// (the text source of truth) with the optional warm-start `cached_index`
/// (a `FileIndex` loaded from the on-disk cache). `file_index` keys on this so
/// the cached fast-path survives the merge; an edit updates `source`'s text and
/// clears `cached_index`, both invalidating `file_index`.
#[salsa::input]
pub struct LspWsFile {
    pub source: SourceFile,
    pub cached_index: Option<Arc<FileIndex>>,
}

/// php-lsp's workspace scoping input on the converged db: the set of project
/// files to aggregate into the workspace index (mir's stub/vendor source files
/// are intentionally excluded). Changes only on file add/remove.
#[salsa::input]
pub struct LspWorkspace {
    pub files: Arc<[LspWsFile]>,
}

/// Parse the file's source text into an arena-backed `ParsedDoc`, memoized on
/// the converged db. Counterpart to [`crate::db::parse::parsed_doc`] keyed on
/// mir's `SourceFile`.
#[salsa::tracked(no_eq, lru = 2048)]
pub fn parsed_doc(db: &dyn MirDatabase, file: SourceFile) -> ParsedArc {
    ParsedArc(Arc::new(ParsedDoc::parse(file.text(db).clone())))
}

/// Build the symbol map for a file. Shares the converged-db [`parsed_doc`], so
/// a file open via `get_doc_salsa` and queried for completion parses once.
#[salsa::tracked(no_eq, lru = 2048)]
pub fn symbol_map(db: &dyn MirDatabase, file: SourceFile) -> SymbolMapArc {
    let doc = parsed_doc(db, file);
    SymbolMapArc(Arc::new(SymbolMap::build(doc.get())))
}

/// Compact per-file declaration index. Warm-start fast path: if a disk-cached
/// index was seeded onto `wf`, return it without parsing. Otherwise share the
/// converged-db [`parsed_doc`] and extract.
#[salsa::tracked]
pub fn file_index(db: &dyn MirDatabase, wf: LspWsFile) -> IndexArc {
    if let Some(cached) = wf.cached_index(db) {
        return IndexArc(cached.clone());
    }
    let doc = parsed_doc(db, *wf.source(db));
    IndexArc(Arc::new(FileIndex::extract(doc.get())))
}

/// Aggregate workspace index over the project files in `ws`. Depends on each
/// file's [`file_index`]; salsa invalidates only the touched file's contribution
/// on an edit.
#[salsa::tracked(no_eq)]
pub fn workspace_index(db: &dyn MirDatabase, ws: LspWorkspace) -> WorkspaceIndexArc {
    let ws_files = ws.files(db);
    let mut files: Vec<(Url, Arc<FileIndex>)> = Vec::with_capacity(ws_files.len());
    for wf in ws_files.iter() {
        let Ok(url) = Url::parse(wf.source(db).path(db)) else {
            continue;
        };
        let idx = file_index(db, *wf).0.clone();
        files.push((url, idx));
    }
    WorkspaceIndexArc(Arc::new(WorkspaceIndexData::from_files(files)))
}
