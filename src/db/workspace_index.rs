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

use salsa::Database;
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

/// What kind of declaration a [`DeclRef`] points at. Drives the per-kind
/// matching rules in [`WorkspaceIndexData::find_declaration`] (e.g. a `$foo`
/// query matches properties and functions/classes named `foo`, but never
/// methods or constants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    Function,
    Class,
    Method,
    Property,
    Constant,
    EnumCase,
}

/// One named declaration, pre-resolved to its file and (zero-width) line.
/// Stored in encounter order — file order, then within a file: functions
/// first, then per class: the class itself, methods, properties, constants,
/// enum cases — which is exactly the precedence the old linear scan in
/// `find_declaration_in_indexes` had.
#[derive(Debug, Clone, Copy)]
pub struct DeclRef {
    pub file: u32,
    pub line: u32,
    pub kind: DeclKind,
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
    /// `declared_name → [DeclRef]` over every function, class, method,
    /// property (stored without `$`), class constant, and enum case in the
    /// workspace. Replaces the O(workspace) linear scan in the go-to-definition
    /// index fallback with an O(1) lookup.
    pub decls_by_name: HashMap<String, Vec<DeclRef>>,
}

type BuildMapsResult = (
    HashMap<String, Vec<ClassRef>>,
    HashMap<Arc<str>, Vec<ClassRef>>,
    HashMap<String, Vec<DeclRef>>,
);

fn build_maps(files: &[(Url, Arc<FileIndex>)]) -> BuildMapsResult {
    let mut classes_by_name: HashMap<String, Vec<ClassRef>> = HashMap::new();
    let mut subtypes_of: HashMap<Arc<str>, Vec<ClassRef>> = HashMap::new();
    let mut decls_by_name: HashMap<String, Vec<DeclRef>> = HashMap::new();
    let push_decl = |map: &mut HashMap<String, Vec<DeclRef>>,
                     name: &str,
                     file: u32,
                     line: u32,
                     kind: DeclKind| {
        map.entry(name.to_string())
            .or_default()
            .push(DeclRef { file, line, kind });
    };
    for (file_idx, (_, idx)) in files.iter().enumerate() {
        let file_idx = file_idx as u32;
        for f in &idx.functions {
            push_decl(
                &mut decls_by_name,
                &f.name,
                file_idx,
                f.start_line,
                DeclKind::Function,
            );
        }
        for (cls_idx, cls) in idx.classes.iter().enumerate() {
            let cr = ClassRef {
                file: file_idx,
                class: cls_idx as u32,
            };
            classes_by_name
                .entry(cls.name.as_ref().to_string())
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
            push_decl(
                &mut decls_by_name,
                &cls.name,
                file_idx,
                cls.start_line,
                DeclKind::Class,
            );
            for m in &cls.methods {
                push_decl(
                    &mut decls_by_name,
                    &m.name,
                    file_idx,
                    m.start_line,
                    DeclKind::Method,
                );
            }
            for p in &cls.properties {
                push_decl(
                    &mut decls_by_name,
                    &p.name,
                    file_idx,
                    p.start_line,
                    DeclKind::Property,
                );
            }
            for cc in &cls.constants {
                push_decl(
                    &mut decls_by_name,
                    cc,
                    file_idx,
                    cls.start_line,
                    DeclKind::Constant,
                );
            }
            for case in &cls.cases {
                push_decl(
                    &mut decls_by_name,
                    case,
                    file_idx,
                    cls.start_line,
                    DeclKind::EnumCase,
                );
            }
        }
    }
    (classes_by_name, subtypes_of, decls_by_name)
}

impl WorkspaceIndexData {
    /// Resolve a `ClassRef` back to its `(uri, class_def)` pair.
    pub fn at(&self, r: ClassRef) -> Option<(&Url, &crate::file_index::ClassDef)> {
        let (uri, idx) = self.files.get(r.file as usize)?;
        let cls = idx.classes.get(r.class as usize)?;
        Some((uri, cls))
    }

    /// O(1) replacement for the linear `find_declaration_in_indexes` scan:
    /// find a declaration by name, optionally excluding one file (the current
    /// document, which the caller has already searched with accurate AST
    /// ranges). Matching rules mirror the old scan: a sigil query (`$foo`)
    /// matches functions, classes, and properties named `foo`; a bare query
    /// matches every declaration kind. Returns a zero-width line `Location`.
    pub fn find_declaration(
        &self,
        name: &str,
        exclude: Option<&Url>,
    ) -> Option<tower_lsp::lsp_types::Location> {
        let bare = crate::util::strip_variable_sigil(name);
        let sigil = bare != name;
        let refs = self.decls_by_name.get(bare)?;
        for r in refs {
            if sigil
                && !matches!(
                    r.kind,
                    DeclKind::Function | DeclKind::Class | DeclKind::Property
                )
            {
                continue;
            }
            let (uri, _) = self.files.get(r.file as usize)?;
            if exclude.is_some_and(|e| e == uri) {
                continue;
            }
            return Some(tower_lsp::lsp_types::Location {
                uri: uri.clone(),
                range: crate::util::zero_width_range(r.line),
            });
        }
        None
    }

    /// Constructor that builds the reverse maps from an already-materialised
    /// `(Url, Arc<FileIndex>)` slice. Exposed so callers that don't want to
    /// spin up a full `AnalysisHost` (unit tests of
    /// `find_implementations_from_workspace`, benchmark crates) can exercise
    /// the aggregate-shaped helpers directly. Production code goes through
    /// the `workspace_index` salsa query instead.
    pub fn from_files(files: Vec<(Url, Arc<FileIndex>)>) -> Self {
        let (classes_by_name, subtypes_of, decls_by_name) = build_maps(&files);
        Self {
            files,
            classes_by_name,
            subtypes_of,
            decls_by_name,
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
crate::impl_arc_update!(WorkspaceIndexArc);

/// Build the aggregate workspace index.
///
/// Depends on `Workspace::files` and every per-file `file_index` query;
/// editing any file invalidates this via normal salsa dependency tracking.
/// The `Arc<FileIndex>` values are the same ones served by `file_index` —
/// callers that already held one are guaranteed pointer-equality here.
#[salsa::tracked(no_eq)]
pub fn workspace_index(db: &dyn Database, ws: Workspace) -> WorkspaceIndexArc {
    let files_input = crate::db::input::workspace_files(db, ws);

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

    let (classes_by_name, subtypes_of, decls_by_name) = build_maps(&files);

    WorkspaceIndexArc(Arc::new(WorkspaceIndexData {
        files,
        classes_by_name,
        subtypes_of,
        decls_by_name,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::db::analysis::AnalysisHost;
    use crate::db::input::FileText;
    use salsa::Setter;

    fn new_file(host: &AnalysisHost, uri: &str, src: &str) -> (Arc<str>, FileText) {
        let ft = FileText::new(host.db(), Arc::<str>::from(src), None);
        (Arc::<str>::from(uri), ft)
    }

    #[test]
    fn workspace_index_builds_name_and_subtype_maps() {
        let host = AnalysisHost::new();
        let f1 = new_file(&host, "file:///a.php", "<?php\nclass Animal {}");
        let f2 = new_file(
            &host,
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
        assert!(names.iter().any(|n| n.as_ref() == "Dog"));
        assert!(names.iter().any(|n| n.as_ref() == "Cat"));
    }

    #[test]
    fn workspace_index_memoizes_and_invalidates() {
        let mut host = AnalysisHost::new();
        let (uri_arc, ft1) = new_file(&host, "file:///a.php", "<?php\nclass A {}");
        let ws = Workspace::new(
            host.db(),
            Arc::from([(uri_arc, ft1)]),
            mir_analyzer::PhpVersion::LATEST,
        );

        let a = workspace_index(host.db(), ws);
        let b = workspace_index(host.db(), ws);
        assert!(
            Arc::ptr_eq(&a.0, &b.0),
            "unchanged inputs must return the memoized Arc"
        );

        ft1.set_text(host.db_mut())
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
        let f = new_file(&host, "file:///m.php", src);
        let ws = Workspace::new(host.db(), Arc::from([f]), mir_analyzer::PhpVersion::LATEST);
        let wi = workspace_index(host.db(), ws);
        let data = wi.get();

        let greeter_subs = data.subtypes_of.get("Greeter").expect("Greeter subs");
        assert_eq!(greeter_subs.len(), 1);
        let shouting_subs = data.subtypes_of.get("Shouting").expect("Shouting subs");
        assert_eq!(shouting_subs.len(), 1);
    }
}
