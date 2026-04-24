//! Database + Analysis/AnalysisHost split (rust-analyzer pattern).
//!
//! `AnalysisHost` owns the mutable database; LSP write paths (`did_open`,
//! `did_change`, workspace scan) go through the host. `Analysis` is a read-only
//! view used by request handlers. Phase A keeps this minimal — cancellation and
//! true snapshot semantics land in Phase E.

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

    /// Produce a read-only analysis view. In Phase A this is a cheap clone of
    /// the database; Phase E will replace this with proper snapshot semantics.
    pub fn analysis(&self) -> Analysis {
        Analysis {
            db: self.db.clone(),
        }
    }
}

/// Read-only view of the database used by LSP request handlers. Reserved
/// for Phase E — request handlers currently go through `DocumentStore`'s
/// salsa-backed accessors instead.
pub struct Analysis {
    db: RootDatabase,
}

impl Analysis {
    pub fn db(&self) -> &RootDatabase {
        &self.db
    }
}
