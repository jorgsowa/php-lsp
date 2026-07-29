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
//!
//! Vocabulary note: the `-Ref` types here (`ClassRef`, `DeclRef`) are internal
//! back-pointers/handles into the index — *not* LSP references. A symbol usage
//! in code is a "reference" spelled out (see `navigation/references.rs`). See
//! the crate-root glossary in `lib.rs`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tower_lsp::lsp_types::Url;

use crate::index::file_index::FileIndex;

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
/// enum cases — which is exactly the precedence the old linear scan (since
/// removed in favor of this map) had.
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
    /// workspace; enables O(1) go-to-definition lookups.
    pub decls_by_name: HashMap<String, Vec<DeclRef>>,
    /// One `(lowercase_short_name, ClassRef)` pair per distinct class name,
    /// sorted by the lowercase key. Built once per revision so completion's
    /// prefix search can binary-search a range instead of scanning every
    /// class name (and re-lowercasing it) on every keystroke.
    pub classes_by_lowercase_name: Vec<(Box<str>, ClassRef)>,
    /// Lazily-built `name/ClassName::method → FuncSignature` map covering
    /// every workspace function and method, for `inlay_hint`'s cross-file
    /// fallback. Built at most once per revision (first `inlay_hint` request
    /// after an edit) and shared by every subsequent request against the
    /// same `WorkspaceIndexData` Arc via [`WorkspaceIndexData::func_signatures`].
    func_signatures: OnceLock<Arc<HashMap<String, FuncSignature>>>,
}

/// A function or method's parameter names, variadic-ness, and return type —
/// enough to drive inlay-hint rendering at a call site without re-parsing
/// the declaring file.
#[derive(Clone)]
pub struct FuncSignature {
    pub params: Vec<String>,
    pub variadic_last: bool,
    pub return_type: Option<String>,
}

/// Populate `map` with function and method signatures from every workspace
/// file. Entries already present (from the current file's own AST) are not
/// overwritten so an in-file definition always wins over a possibly-stale
/// index entry.
pub fn build_func_signatures(
    files: &[(Url, Arc<FileIndex>)],
) -> HashMap<String, FuncSignature> {
    let mut map = HashMap::new();
    for (_, idx) in files {
        for func in &idx.functions {
            let func_name = func.name.to_string();
            if map.contains_key(&func_name) {
                continue;
            }
            let params: Vec<String> = func.params.iter().map(|p| p.name.to_string()).collect();
            let variadic_last = func.params.last().map(|p| p.variadic).unwrap_or(false);
            map.insert(
                func_name,
                FuncSignature {
                    params,
                    variadic_last,
                    return_type: func.return_type.as_ref().map(|r| r.to_string()),
                },
            );
        }
        for class in &idx.classes {
            for method in &class.methods {
                let method_name = method.name.to_string();
                let params: Vec<String> =
                    method.params.iter().map(|p| p.name.to_string()).collect();
                let variadic_last = method.params.last().map(|p| p.variadic).unwrap_or(false);
                let sig = FuncSignature {
                    params: params.clone(),
                    variadic_last,
                    return_type: method.return_type.as_ref().map(|r| r.to_string()),
                };
                // Register with qualified key "ClassName::methodName" for unambiguous lookup
                let cn = class.name.as_ref();
                let qualified = format!("{}::{}", cn, method_name);
                map.insert(qualified, sig.clone());
                // Also register __construct under the class name so `new ClassName(...)` gets hints.
                if method_name == "__construct" {
                    map.entry(cn.to_string()).or_insert_with(|| FuncSignature {
                        params: params.clone(),
                        variadic_last,
                        return_type: None,
                    });
                }
                // Register with short name as fallback for backwards compatibility
                map.entry(method_name).or_insert(sig);
            }
        }
    }
    map
}

pub(crate) type BuildMapsResult = (
    HashMap<String, Vec<ClassRef>>,
    HashMap<Arc<str>, Vec<ClassRef>>,
    HashMap<String, Vec<DeclRef>>,
    Vec<(Box<str>, ClassRef)>,
);

pub(crate) fn build_maps(files: &[(Url, Arc<FileIndex>)]) -> BuildMapsResult {
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
                // If this implements name is a use-import alias, also index by
                // the short name of the resolved FQN so cursor-on-interface-name
                // lookups work regardless of how the implementor named the interface.
                if let Some((_, fqn)) = idx
                    .use_imports
                    .iter()
                    .find(|(alias, _)| alias.as_ref() == iface.as_ref())
                {
                    let short = crate::text::fqn_short_name(fqn);
                    if short != iface.as_ref() {
                        subtypes_of.entry(Arc::from(short)).or_default().push(cr);
                    }
                }
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
    let mut classes_by_lowercase_name: Vec<(Box<str>, ClassRef)> = classes_by_name
        .iter()
        .filter_map(|(name, refs)| refs.first().map(|cr| (name.to_lowercase().into(), *cr)))
        .collect();
    classes_by_lowercase_name.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    (
        classes_by_name,
        subtypes_of,
        decls_by_name,
        classes_by_lowercase_name,
    )
}

impl WorkspaceIndexData {
    /// Resolve a `ClassRef` back to its `(uri, class_def)` pair.
    pub fn at(&self, r: ClassRef) -> Option<(&Url, &crate::index::file_index::ClassDef)> {
        let (uri, idx) = self.files.get(r.file as usize)?;
        let cls = idx.classes.get(r.class as usize)?;
        Some((uri, cls))
    }

    /// Resolve `name` to a single `ClassRef`, disambiguating same-named
    /// classes in different namespaces. `classes_by_name` is keyed by short
    /// name only, so two unrelated classes sharing a name (e.g. Laravel's
    /// `Illuminate\Support\Facades\Auth` vs `Illuminate\Container\Attributes\
    /// Auth`) collide in one bucket; picking `.first()` unconditionally
    /// silently returns whichever happens to be indexed first.
    ///
    /// `name` may be a bare short name (kept for compatibility — resolves to
    /// the first match, as before) or a fully-qualified name (`Foo\Bar\Baz`,
    /// optionally leading with `\`), in which case the candidate whose own
    /// FQN matches (case-insensitively, PHP class names are
    /// case-insensitive) is preferred over an arbitrary first match.
    pub fn resolve_class_ref(&self, name: &str) -> Option<ClassRef> {
        let trimmed = name.trim_start_matches('\\');
        let short = trimmed.rsplit('\\').next().unwrap_or(trimmed);
        let candidates = self.classes_by_name.get(short)?;
        if trimmed.contains('\\')
            && let Some(cr) = candidates.iter().find(|cr| {
                self.at(**cr).is_some_and(|(_, cls)| {
                    cls.fqn
                        .trim_start_matches('\\')
                        .eq_ignore_ascii_case(trimmed)
                })
            })
        {
            return Some(*cr);
        }
        candidates.first().copied()
    }

    /// O(1) replacement for the old linear per-file scan (since removed):
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
        let bare = crate::text::strip_variable_sigil(name);
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
                range: crate::text::zero_width_range(r.line),
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
        let (classes_by_name, subtypes_of, decls_by_name, classes_by_lowercase_name) =
            build_maps(&files);
        Self {
            files,
            classes_by_name,
            subtypes_of,
            decls_by_name,
            classes_by_lowercase_name,
            func_signatures: OnceLock::new(),
        }
    }

    /// The workspace-wide function/method signature map, built on first use
    /// per revision and shared (via `Arc` clone) by every later `inlay_hint`
    /// request against this same `WorkspaceIndexData`.
    pub fn func_signatures(&self) -> Arc<HashMap<String, FuncSignature>> {
        Arc::clone(
            self.func_signatures
                .get_or_init(|| Arc::new(build_func_signatures(&self.files))),
        )
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
