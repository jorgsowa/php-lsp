//! Database + Analysis/AnalysisHost split (rust-analyzer pattern).
//!
//! `AnalysisHost` owns the mutable database; LSP write paths (`did_open`,
//! `did_change`, workspace scan) go through the host. `Analysis` is a read-only
//! view used by request handlers.

use std::sync::{Arc, Mutex};

use mir_analyzer::PhpVersion;
use mir_analyzer::db::MirDb;
use mir_codebase::storage::StubSlice;
use salsa::{Database, Storage};

/// Cache for the workspace-wide populated `MirDb`. Built once per
/// `(slices_arc, php_version)` pair and reused across every salsa query that
/// needs MirDb access (`semantic_issues`, `file_refs`, `class_issues`) plus
/// `Backend::codebase()`.
///
/// Without this cache, each salsa query rebuilds the MirDb from scratch
/// (`load_stubs + ingest × N files`). On a workspace-wide diagnostic pass
/// after one edit, that's `O(N²)` work because every per-file `semantic_issues`
/// is invalidated and rebuilds independently. With the cache, the rebuild
/// happens once per workspace edit; subsequent queries clone (cheap shallow
/// `Arc<FxHashMap>` clones).
///
/// Held inside `Mutex` because `MirDb: Send` but `!Sync` (its salsa storage
/// contains per-thread `RefCell`s). The `Arc` makes the cache shared across
/// every snapshot clone of `RootDatabase`.
type MirDbCache = Arc<Mutex<Option<(Arc<[Arc<StubSlice>]>, PhpVersion, MirDb)>>>;

#[salsa::db]
#[derive(Default, Clone)]
pub struct RootDatabase {
    storage: Storage<Self>,
    mir_db_cache: MirDbCache,
}

#[salsa::db]
impl Database for RootDatabase {}

/// Extension trait providing a workspace-wide populated `MirDb` to salsa
/// queries. Tracked queries that need MirDb access take `&dyn LspDatabase`
/// instead of `&dyn salsa::Database` so they can hit the shared cache.
#[salsa::db]
pub trait LspDatabase: Database {
    fn cached_mir_db(&self, slices: Arc<[Arc<StubSlice>]>, php_version: PhpVersion) -> MirDb;
}

#[salsa::db]
impl LspDatabase for RootDatabase {
    fn cached_mir_db(&self, slices: Arc<[Arc<StubSlice>]>, php_version: PhpVersion) -> MirDb {
        let mut cache = self.mir_db_cache.lock().unwrap();
        if let Some((cached_slices, cached_ver, cached_db)) = cache.as_ref()
            && Arc::ptr_eq(cached_slices, &slices)
            && *cached_ver == php_version
        {
            return cached_db.clone();
        }
        let mut fresh = MirDb::default();
        mir_analyzer::stubs::load_stubs_for_version(&mut fresh, php_version);
        for slice in slices.iter() {
            fresh.ingest_stub_slice(slice);
        }
        let out = fresh.clone();
        *cache = Some((slices, php_version, fresh));
        out
    }
}

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
