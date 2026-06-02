//! Database + AnalysisHost split (rust-analyzer pattern).
//!
//! `AnalysisHost` owns the mutable salsa database; LSP write paths
//! (`did_open`, `did_change`, workspace scan) go through the host.
//! Read-only handlers snapshot the db (cheap `Arc<Zalsa>` clone) and run
//! queries lock-free.
//!
//! After the mir 0.22 migration, this module no longer owns the workspace
//! `MirDb` — that's the responsibility of `mir_analyzer::AnalysisSession`
//! held by `DocumentStore`. Salsa is for parsed_doc / file_index only.

use salsa::{Database, Storage};

#[salsa::db]
#[derive(Default, Clone)]
pub struct RootDatabase {
    storage: Storage<Self>,
}

#[salsa::db]
impl Database for RootDatabase {}

/// Owns the mutable salsa database. Backend will hold one of these.
#[derive(Default)]
pub struct AnalysisHost {
    db: RootDatabase,
}

impl AnalysisHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn db(&self) -> &RootDatabase {
        &self.db
    }

    pub fn db_mut(&mut self) -> &mut RootDatabase {
        &mut self.db
    }
}
