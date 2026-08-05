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
//! This query runs once per workspace revision and returns `files`: the flat
//! `(Uri, Arc<FileIndex>)` list every handler used to rebuild by hand, plus
//! `classes_by_lowercase_name` for completion's prefix search.
//!
//! All lookups on the aggregate run in memory against an already-materialised
//! `Arc`; edits invalidate the aggregate through `file_index` dependency
//! tracking as usual.
//!
//! Vocabulary note: `ClassRef` is an internal back-pointer/handle into the
//! index — *not* an LSP reference. A symbol usage in code is a "reference"
//! spelled out (see `navigation/references.rs`). See the crate-root glossary
//! in `lib.rs`.
//!
//! Short-name class lookup and disambiguation (`DocumentStore::
//! resolve_class_ref`/`class_candidates`) and arbitrary-name declaration
//! lookup (`DocumentStore::declaration_candidate_files`/`find_declaration`)
//! no longer have eagerly rebuilt-per-revision maps here. Both narrow
//! candidates via mir's persistent per-file mention cache instead: an edit
//! invalidates only that file's cached mention set, not the whole
//! workspace's. Only `classes_by_lowercase_name` remains eager — prefix
//! search genuinely needs to enumerate every distinct name, which a
//! point-query mention cache can't answer.

use std::collections::HashMap;
use std::sync::Arc;

use tower_lsp_server::ls_types::Uri;

use crate::index::file_index::FileIndex;

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
    pub files: Vec<(Uri, Arc<FileIndex>)>,
    /// `files`' URIs as mir path strings, precomputed once per revision so
    /// `DocumentStore::declaration_candidate_files`/`class_candidates` don't
    /// re-allocate one `Arc<str>` per file on every call.
    pub(crate) file_paths: Vec<Arc<str>>,
    /// `file_paths[i] → i`, for O(1) resolution of a mention-index candidate
    /// path back to its `files`/`file_paths` index (no per-candidate linear
    /// scan or `Uri` round-trip).
    pub(crate) path_to_file_idx: HashMap<Arc<str>, u32>,
    /// One `(lowercase_short_name, ClassRef)` pair per distinct class name,
    /// sorted by the lowercase key. Built once per revision so completion's
    /// prefix search can binary-search a range instead of scanning every
    /// class name (and re-lowercasing it) on every keystroke.
    pub classes_by_lowercase_name: Vec<(Box<str>, ClassRef)>,
}

/// One entry per distinct class name (first-encountered `ClassRef` wins —
/// matches the old `classes_by_name`-derived construction), for
/// [`WorkspaceIndexData::classes_by_lowercase_name`]'s sorted table. No
/// longer builds a full name→candidates map or a subtype reverse map: every
/// other caller now resolves those via mir's mention index
/// (`DocumentStore::class_candidates`/`resolve_class_ref`/
/// `declaration_candidate_files`) instead of an eagerly rebuilt structure.
pub(crate) fn build_maps(files: &[(Uri, Arc<FileIndex>)]) -> Vec<(Box<str>, ClassRef)> {
    let mut first_by_name: HashMap<Box<str>, ClassRef> = HashMap::new();
    for (file_idx, (_, idx)) in files.iter().enumerate() {
        let file_idx = file_idx as u32;
        for (cls_idx, cls) in idx.classes.iter().enumerate() {
            first_by_name
                .entry(cls.name.as_ref().into())
                .or_insert(ClassRef {
                    file: file_idx,
                    class: cls_idx as u32,
                });
        }
    }
    let mut classes_by_lowercase_name: Vec<(Box<str>, ClassRef)> = first_by_name
        .into_iter()
        .map(|(name, cr)| (name.to_lowercase().into(), cr))
        .collect();
    classes_by_lowercase_name.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    classes_by_lowercase_name
}

impl WorkspaceIndexData {
    /// Resolve a `ClassRef` back to its `(uri, class_def)` pair.
    pub fn at(&self, r: ClassRef) -> Option<(&Uri, &crate::index::file_index::ClassDef)> {
        let (uri, idx) = self.files.get(r.file as usize)?;
        let cls = idx.classes.get(r.class as usize)?;
        Some((uri, cls))
    }

    /// Visit every class-like declaration stored in the aggregate.
    pub fn for_each_class(
        &self,
        mut f: impl FnMut(&Uri, &crate::index::file_index::ClassDef),
    ) {
        for (uri, idx) in &self.files {
            for cls in &idx.classes {
                f(uri, cls);
            }
        }
    }

    /// Visit every class-like declaration in the files named by `uris`.
    /// Missing paths are skipped; callers typically source `uris` from mir's
    /// mention or subtype indexes, which are candidate sets rather than proof.
    pub fn for_each_class_in_uris(
        &self,
        uris: &[Uri],
        mut f: impl FnMut(&Uri, &crate::index::file_index::ClassDef),
    ) {
        for uri in uris {
            let Some(&file_idx) = self.path_to_file_idx.get(uri.as_str()) else {
                continue;
            };
            let Some((stored_uri, idx)) = self.files.get(file_idx as usize) else {
                continue;
            };
            for cls in &idx.classes {
                f(stored_uri, cls);
            }
        }
    }

    /// Constructor that builds the reverse maps from an already-materialised
    /// `(Uri, Arc<FileIndex>)` slice. Exposed so callers that don't want to
    /// spin up a full `AnalysisHost` (unit tests of
    /// `find_implementations_from_workspace`, benchmark crates) can exercise
    /// the aggregate-shaped helpers directly. Production code goes through
    /// the `workspace_index` salsa query instead.
    pub fn from_files(files: Vec<(Uri, Arc<FileIndex>)>) -> Self {
        let classes_by_lowercase_name = build_maps(&files);
        let file_paths: Vec<Arc<str>> = files.iter().map(|(u, _)| Arc::from(u.as_str())).collect();
        let path_to_file_idx = file_paths
            .iter()
            .enumerate()
            .map(|(i, p)| (Arc::clone(p), i as u32))
            .collect();
        Self {
            files,
            file_paths,
            path_to_file_idx,
            classes_by_lowercase_name,
        }
    }
}

/// Arc wrapper for `workspace_index`, which is `#[salsa::tracked(no_eq)]` —
/// every rebuild allocates a fresh `Arc` and salsa never attempts to compare
/// `WorkspaceIndexData` structurally.
#[derive(Clone)]
pub struct WorkspaceIndexArc(pub Arc<WorkspaceIndexData>);

impl WorkspaceIndexArc {
    #[cfg(test)]
    pub fn get(&self) -> &WorkspaceIndexData {
        &self.0
    }
}
