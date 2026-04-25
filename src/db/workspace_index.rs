//! `workspace_index` salsa query — aggregates every file's `FileIndex` into a
//! single structure with pre-built reverse maps.
//!
//! Before Phase J, cross-file queries (`workspace_symbols`,
//! `prepare_type_hierarchy`, `supertypes_of`, `subtypes_of`,
//! `find_implementations`) called `DocumentStore::all_indexes()` on every
//! request. `all_indexes()` takes the host mutex once per file via
//! `get_index_salsa` → `snapshot_query`, so a workspace with 1600 files
//! paid 1600 lock acquisitions per lookup.
//!
//! This query runs once per workspace revision and returns:
//!
//! - `files`: the flat `(Url, Arc<FileIndex>)` list every handler used to
//!   rebuild by hand,
//! - `classes_by_name`: `name → [ClassRef]` for constant-time prepare /
//!   supertype resolution,
//! - `subtypes_of`: `name → [ClassRef]` for constant-time subtype /
//!   implementation lookups.
//!
//! All lookups on the aggregate run in memory against an already-materialised
//! `Arc`; edits invalidate the aggregate through `file_index` dependency
//! tracking as usual.

use std::collections::HashMap;
use std::sync::Arc;

use salsa::{Database, Update};
use tower_lsp::lsp_types::Url;

use crate::db::index::file_index;
use crate::db::input::Workspace;
use crate::file_index::FileIndex;

/// Back-pointer into `WorkspaceIndexData.files`: `(file_idx, class_idx)` where
/// `class_idx` indexes into `files[file_idx].1.classes`.
#[derive(Debug, Clone, Copy)]
pub struct ClassRef {
    pub file: u32,
    pub class: u32,
}

/// Aggregated workspace-level index. Constructed once per salsa revision by
/// `workspace_index` and held behind an `Arc` for cheap cross-request sharing.
pub struct WorkspaceIndexData {
    pub files: Vec<(Url, Arc<FileIndex>)>,
    pub classes_by_name: HashMap<String, Vec<ClassRef>>,
    /// `parent_or_interface_or_trait_name → [subtype ClassRef]`.
    /// A class that extends `X` AND implements `Y` contributes separate entries
    /// under both keys. Keyed by `Arc<str>` so insertions from `ClassDef`'s
    /// already-interned fields are pointer copies rather than heap allocations.
    pub subtypes_of: HashMap<Arc<str>, Vec<ClassRef>>,
}

impl WorkspaceIndexData {
    /// Resolve a `ClassRef` back to its `(uri, class_def)` pair.
    pub fn at(&self, r: ClassRef) -> Option<(&Url, &crate::file_index::ClassDef)> {
        let (uri, idx) = self.files.get(r.file as usize)?;
        let cls = idx.classes.get(r.class as usize)?;
        Some((uri, cls))
    }

    /// Test-only constructor that builds `classes_by_name` + `subtypes_of`
    /// from an already-materialised `(Url, Arc<FileIndex>)` slice. Exposed
    /// so callers that don't want to spin up a full `AnalysisHost` (unit
    /// tests of `find_implementations_from_workspace` etc.) can exercise
    /// the aggregate-shaped helpers directly.
    #[cfg(test)]
    pub fn from_files(files: Vec<(Url, Arc<FileIndex>)>) -> Self {
        let mut classes_by_name: HashMap<String, Vec<ClassRef>> = HashMap::new();
        let mut subtypes_of: HashMap<Arc<str>, Vec<ClassRef>> = HashMap::new();
        for (file_idx, (_, idx)) in files.iter().enumerate() {
            let file_idx = file_idx as u32;
            for (cls_idx, cls) in idx.classes.iter().enumerate() {
                let cr = ClassRef {
                    file: file_idx,
                    class: cls_idx as u32,
                };
                classes_by_name
                    .entry(cls.name.clone())
                    .or_default()
                    .push(cr);
                if let Some(parent) = &cls.parent {
                    subtypes_of.entry(Arc::clone(parent)).or_default().push(cr);
                }
                for iface in &cls.implements {
                    subtypes_of.entry(Arc::clone(iface)).or_default().push(cr);
                }
                for trt in &cls.traits {
                    subtypes_of.entry(Arc::clone(trt)).or_default().push(cr);
                }
            }
        }
        Self {
            files,
            classes_by_name,
            subtypes_of,
        }
    }
}

/// Arc wrapper with the same `Arc::ptr_eq`-based `Update` impl used throughout
/// `src/db/`. The inner `WorkspaceIndexData` never compares structurally.
#[derive(Clone)]
pub struct WorkspaceIndexArc(pub Arc<WorkspaceIndexData>);

impl WorkspaceIndexArc {
    #[cfg(test)]
    pub fn get(&self) -> &WorkspaceIndexData {
        &self.0
    }
}

// SAFETY: same contract as other `*Arc` newtypes — ptr_eq is sufficient because
// every rebuild allocates a fresh `Arc`.
unsafe impl Update for WorkspaceIndexArc {
    unsafe fn maybe_update(old_pointer: *mut Self, new_value: Self) -> bool {
        let old_ref = unsafe { &mut *old_pointer };
        if Arc::ptr_eq(&old_ref.0, &new_value.0) {
            false
        } else {
            *old_ref = new_value;
            true
        }
    }
}

/// Build the aggregate workspace index.
///
/// Depends on `Workspace::files` and every per-file `file_index` query;
/// editing any file invalidates this via normal salsa dependency tracking.
/// The `Arc<FileIndex>` values are the same ones served by `file_index` —
/// callers that already held one are guaranteed pointer-equality here.
#[salsa::tracked(no_eq)]
pub fn workspace_index(db: &dyn Database, ws: Workspace) -> WorkspaceIndexArc {
    let files_input = ws.files(db);

    let mut files: Vec<(Url, Arc<FileIndex>)> = Vec::with_capacity(files_input.len());
    for sf in files_input.iter() {
        let uri_arc = sf.uri(db);
        // Fall back to inserting any well-formed entry — salsa inputs carry
        // whatever string the caller mirrored; if parsing ever fails (test
        // harness with a non-URL string) we simply skip that file for
        // cross-workspace queries rather than panic.
        let Ok(url) = Url::parse(&uri_arc) else {
            continue;
        };
        let idx = file_index(db, *sf).0.clone();
        files.push((url, idx));
    }

    let mut classes_by_name: HashMap<String, Vec<ClassRef>> = HashMap::new();
    let mut subtypes_of: HashMap<Arc<str>, Vec<ClassRef>> = HashMap::new();

    for (file_idx, (_, idx)) in files.iter().enumerate() {
        let file_idx = file_idx as u32;
        for (cls_idx, cls) in idx.classes.iter().enumerate() {
            let cr = ClassRef {
                file: file_idx,
                class: cls_idx as u32,
            };
            classes_by_name
                .entry(cls.name.clone())
                .or_default()
                .push(cr);
            if let Some(parent) = &cls.parent {
                subtypes_of.entry(Arc::clone(parent)).or_default().push(cr);
            }
            for iface in &cls.implements {
                subtypes_of.entry(Arc::clone(iface)).or_default().push(cr);
            }
            for trt in &cls.traits {
                subtypes_of.entry(Arc::clone(trt)).or_default().push(cr);
            }
        }
    }

    WorkspaceIndexArc(Arc::new(WorkspaceIndexData {
        files,
        classes_by_name,
        subtypes_of,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::{FileId, SourceFile};
    use salsa::Setter;

    fn new_file(host: &AnalysisHost, id: u32, uri: &str, src: &str) -> SourceFile {
        SourceFile::new(
            host.db(),
            FileId(id),
            Arc::<str>::from(uri),
            Arc::<str>::from(src),
            None,
        )
    }

    #[test]
    fn workspace_index_builds_name_and_subtype_maps() {
        let host = AnalysisHost::new();
        let f1 = new_file(&host, 0, "file:///a.php", "<?php\nclass Animal {}");
        let f2 = new_file(
            &host,
            1,
            "file:///b.php",
            "<?php\nclass Dog extends Animal {}\nclass Cat extends Animal {}",
        );
        let ws = Workspace::new(
            host.db(),
            Arc::from([f1, f2]),
            mir_analyzer::PhpVersion::LATEST,
        );

        let wi = workspace_index(host.db(), ws);
        let data = wi.get();

        assert!(data.classes_by_name.contains_key("Animal"));
        assert!(data.classes_by_name.contains_key("Dog"));

        let subs = data
            .subtypes_of
            .get("Animal")
            .expect("Animal must have subtype entries");
        assert_eq!(subs.len(), 2, "Dog + Cat extend Animal");

        let names: Vec<_> = subs
            .iter()
            .filter_map(|r| data.at(*r).map(|(_, c)| c.name.clone()))
            .collect();
        assert!(names.contains(&"Dog".to_string()));
        assert!(names.contains(&"Cat".to_string()));
    }

    #[test]
    fn workspace_index_memoizes_and_invalidates() {
        let mut host = AnalysisHost::new();
        let f1 = new_file(&host, 0, "file:///a.php", "<?php\nclass A {}");
        let ws = Workspace::new(host.db(), Arc::from([f1]), mir_analyzer::PhpVersion::LATEST);

        let a = workspace_index(host.db(), ws);
        let b = workspace_index(host.db(), ws);
        assert!(
            Arc::ptr_eq(&a.0, &b.0),
            "unchanged inputs must return the memoized Arc"
        );

        f1.set_text(host.db_mut())
            .to(Arc::<str>::from("<?php\nclass B {}"));
        let c = workspace_index(host.db(), ws);
        assert!(!Arc::ptr_eq(&a.0, &c.0), "an edit must produce a fresh Arc");
        assert!(c.get().classes_by_name.contains_key("B"));
        assert!(!c.get().classes_by_name.contains_key("A"));
    }

    #[test]
    fn workspace_index_collects_interface_and_trait_subtypes() {
        let host = AnalysisHost::new();
        let src = concat!(
            "<?php\n",
            "interface Greeter {}\n",
            "trait Shouting {}\n",
            "class Hi implements Greeter { use Shouting; }\n",
        );
        let f = new_file(&host, 0, "file:///m.php", src);
        let ws = Workspace::new(host.db(), Arc::from([f]), mir_analyzer::PhpVersion::LATEST);
        let wi = workspace_index(host.db(), ws);
        let data = wi.get();

        let greeter_subs = data.subtypes_of.get("Greeter").expect("Greeter subs");
        assert_eq!(greeter_subs.len(), 1);
        let shouting_subs = data.subtypes_of.get("Shouting").expect("Shouting subs");
        assert_eq!(shouting_subs.len(), 1);
    }
}
