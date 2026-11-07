/// Compact symbol index extracted from a parsed PHP file.
///
/// A `FileIndex` captures only the declaration-level information needed for
/// cross-file features (go-to-definition, workspace symbols, hover signatures,
/// find-implementations, etc.).  It is ~2 KB per file compared to ~100 KB for
/// a full `ParsedDoc`, allowing the LSP to keep thousands of background files
/// in memory without exhausting RAM.
///
/// Built from mir's per-file definitions by [`FileIndex::from_definitions`].
///
/// The `-Def` types here (`FunctionDef`, `ClassDef`, `MethodDef`, …) are owned
/// declaration records stored in the index — distinct from the `-Entry` rows of
/// `SymbolMap` and the `-Ref` index handles in `db::workspace_index`. See the
/// crate-root glossary in `lib.rs`.
use std::sync::Arc;

use mir_codebase::definitions::{self as mir, StubSlice};
use mir_types::{Location, Type};
use tower_lsp_server::ls_types::SymbolKind;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FileIndex {
    pub namespace: Option<Box<str>>,
    pub functions: Vec<FunctionDef>,
    pub classes: Vec<ClassDef>,
    pub constants: Vec<Box<str>>,
    /// `workspace/symbol` rows, precomputed so a query never re-parses the file.
    pub symbols: Vec<WorkspaceSymbolRow>,
}

/// One flattened `document_symbols` entry, as `workspace/symbol` reports it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceSymbolRow {
    pub name: Box<str>,
    pub kind: SymbolKind,
    pub line: u32,
    /// Index of the enclosing row in the same `symbols` vec.
    pub container: Option<u32>,
    pub deprecated: Option<bool>,
}

impl FileIndex {
    /// True when the file declares nothing another file could reference —
    /// namespace and imports don't count, only functions/classes/constants.
    pub fn declares_nothing(&self) -> bool {
        self.functions.is_empty() && self.classes.is_empty() && self.constants.is_empty()
    }

    /// Whether `any_declaration_in_doc` could match here, answered without parsing the file.
    /// Non-classes always might: their `@method` tags and hooked properties aren't indexed.
    pub fn may_declare(&self, word: &str, bare: &str) -> bool {
        self.functions.iter().any(|f| &*f.name == word)
            || self.classes.iter().any(|c| {
                c.kind != ClassKind::Class
                    || &*c.name == word
                    || c.methods.iter().any(|m| &*m.name == word)
                    || c.properties.iter().any(|p| &*p.name == bare)
                    || c.constants.iter().any(|k| &**k == word)
                    || c.cases.iter().any(|k| &**k == word)
                    || c.doc_methods
                        .iter()
                        .any(|m| m.name.eq_ignore_ascii_case(word))
            })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FunctionDef {
    pub name: Box<str>,
    /// Fully-qualified name: `\Namespace\function_name` or just `function_name`.
    pub fqn: Box<str>,
    pub params: Vec<ParamDef>,
    pub return_type: Option<Box<str>>,
    pub start_line: u32,
    /// Character position of the function name on its line (UTF-16 code units).
    pub name_char: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamDef {
    pub name: Box<str>,
    pub type_hint: Option<Box<str>>,
    pub has_default: bool,
    pub variadic: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClassDef {
    pub name: Box<str>,
    /// Fully-qualified name.
    pub fqn: Box<str>,
    pub kind: ClassKind,
    pub is_abstract: bool,
    /// `true` for `readonly class Foo {}` (PHP 8.2+).
    pub is_readonly: bool,
    /// `extends` clause as written in source (may be short name or FQN).
    pub parent: Option<Arc<str>>,
    pub implements: Vec<Arc<str>>,
    pub traits: Vec<Arc<str>>,
    pub methods: Vec<MethodDef>,
    pub properties: Vec<PropertyDef>,
    pub constants: Vec<Box<str>>,
    /// Enum case names (only populated for `ClassKind::Enum`).
    pub cases: Vec<Box<str>>,
    pub start_line: u32,
    /// Character position of the class/interface/trait/enum name on its line (UTF-16 code units).
    pub name_char: u32,
    /// Virtual methods declared via `@method` docblock tags.
    pub doc_methods: Vec<DocMethodEntry>,
    /// Classes/traits pulled in via `@mixin ClassName` docblock tags.
    pub mixins: Vec<Arc<str>>,
    /// Method aliases introduced by `use Trait { method as alias; }` in this class.
    pub trait_method_aliases: Vec<TraitMethodAlias>,
}

/// A trait method alias introduced by `use Trait { method as alias; }`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TraitMethodAlias {
    /// The new name assigned by the alias clause.
    pub alias: Box<str>,
    /// The original method name in the trait.
    pub original: Box<str>,
    /// The specific trait the alias targets, if spelled out (`Trait::method as alias`).
    pub trait_name: Option<Box<str>>,
}

/// A parameter from a `@method` docblock tag.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DocMethodParam {
    pub name: Box<str>,
    pub type_hint: Option<Box<str>>,
    pub is_variadic: bool,
    pub is_byref: bool,
    pub is_optional: bool,
}

/// A method declared only via a `@method` docblock tag (no real body).
/// Kept separate from `MethodDef` so consumers that build signatures or inlay
/// hints don't accidentally iterate over methods with no parameter information.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DocMethodEntry {
    pub name: Box<str>,
    pub is_static: bool,
    /// Return type as written in the `@method` tag, e.g. `"User"` or `"static"`.
    pub return_type: Option<Box<str>>,
    /// Source line of the `@method` tag (0-based).
    pub start_line: u32,
    /// Parameters extracted from the `@method` tag signature.
    pub params: Vec<DocMethodParam>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MethodDef {
    pub name: Box<str>,
    pub is_static: bool,
    pub is_abstract: bool,
    pub visibility: Visibility,
    pub params: Vec<ParamDef>,
    pub return_type: Option<Box<str>>,
    pub start_line: u32,
    /// Character position of the method name on its line (UTF-16 code units).
    pub name_char: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PropertyDef {
    pub name: Box<str>,
    pub is_static: bool,
    pub is_readonly: bool,
    pub type_hint: Option<Box<str>>,
    pub visibility: Visibility,
    pub start_line: u32,
    /// Character position of the property name on its line (UTF-16 code units).
    pub name_char: u32,
}

/// Equality that ignores source positions, so shifting a declaration to
/// another line doesn't count as changing it.
pub trait SameDeclarations {
    fn same_declarations(&self, other: &Self) -> bool;
}

impl<T: SameDeclarations> SameDeclarations for [T] {
    fn same_declarations(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.same_declarations(b))
    }
}

impl SameDeclarations for FileIndex {
    fn same_declarations(&self, other: &Self) -> bool {
        // `symbols` only feeds `workspace/symbol`; the declarations it lists are compared below.
        let FileIndex {
            namespace,
            functions,
            classes,
            constants,
            symbols: _,
        } = self;
        *namespace == other.namespace
            && functions.same_declarations(&other.functions)
            && classes.same_declarations(&other.classes)
            && *constants == other.constants
    }
}

impl SameDeclarations for FunctionDef {
    fn same_declarations(&self, other: &Self) -> bool {
        let FunctionDef {
            name,
            fqn,
            params,
            return_type,
            start_line: _,
            name_char: _,
        } = self;
        *name == other.name
            && *fqn == other.fqn
            && *params == other.params
            && *return_type == other.return_type
    }
}

impl SameDeclarations for ClassDef {
    fn same_declarations(&self, other: &Self) -> bool {
        let ClassDef {
            name,
            fqn,
            kind,
            is_abstract,
            is_readonly,
            parent,
            implements,
            traits,
            methods,
            properties,
            constants,
            cases,
            start_line: _,
            name_char: _,
            doc_methods,
            mixins,
            trait_method_aliases,
        } = self;
        *name == other.name
            && *fqn == other.fqn
            && *kind == other.kind
            && *is_abstract == other.is_abstract
            && *is_readonly == other.is_readonly
            && *parent == other.parent
            && *implements == other.implements
            && *traits == other.traits
            && methods.same_declarations(&other.methods)
            && properties.same_declarations(&other.properties)
            && *constants == other.constants
            && *cases == other.cases
            && doc_methods.same_declarations(&other.doc_methods)
            && *mixins == other.mixins
            && *trait_method_aliases == other.trait_method_aliases
    }
}

impl SameDeclarations for MethodDef {
    fn same_declarations(&self, other: &Self) -> bool {
        let MethodDef {
            name,
            is_static,
            is_abstract,
            visibility,
            params,
            return_type,
            start_line: _,
            name_char: _,
        } = self;
        *name == other.name
            && *is_static == other.is_static
            && *is_abstract == other.is_abstract
            && *visibility == other.visibility
            && *params == other.params
            && *return_type == other.return_type
    }
}

impl SameDeclarations for PropertyDef {
    fn same_declarations(&self, other: &Self) -> bool {
        let PropertyDef {
            name,
            is_static,
            is_readonly,
            type_hint,
            visibility,
            start_line: _,
            name_char: _,
        } = self;
        *name == other.name
            && *is_static == other.is_static
            && *is_readonly == other.is_readonly
            && *type_hint == other.type_hint
            && *visibility == other.visibility
    }
}

impl SameDeclarations for DocMethodEntry {
    fn same_declarations(&self, other: &Self) -> bool {
        let DocMethodEntry {
            name,
            is_static,
            return_type,
            start_line: _,
            params,
        } = self;
        *name == other.name
            && *is_static == other.is_static
            && *return_type == other.return_type
            && *params == other.params
    }
}

impl FileIndex {
    /// Builds the index from mir's per-file definitions; `source` locates declared names.
    pub fn from_definitions(slice: &StubSlice, source: &str) -> Self {
        let text = SourceText::new(source);
        let mut decls: Vec<Decl> = Vec::new();
        for f in &slice.functions {
            decls.push(function_decl(&text, f));
        }
        for c in &slice.classes {
            decls.push(class_decl(&text, c));
        }
        for i in &slice.interfaces {
            decls.push(interface_decl(&text, i));
        }
        for t in &slice.traits {
            decls.push(trait_decl(&text, t));
        }
        for e in &slice.enums {
            decls.push(enum_decl(&text, e));
        }
        decls.sort_by_key(|d| d.key);

        let mut index = FileIndex {
            namespace: slice
                .namespace
                .as_deref()
                .map(|ns| ns.trim_start_matches('\\').into()),
            constants: slice
                .constants
                .iter()
                .map(|(name, _)| short_name(name).into())
                .collect(),
            ..FileIndex::default()
        };
        let mut namespace_rows: Vec<(Box<str>, u32)> = Vec::new();
        for decl in decls {
            let container = decl.namespace.map(|ns| {
                if let Some((_, row)) = namespace_rows.iter().find(|(n, _)| *n == ns) {
                    return *row;
                }
                let row = index.symbols.len() as u32;
                index.symbols.push(WorkspaceSymbolRow {
                    name: ns.clone(),
                    kind: SymbolKind::NAMESPACE,
                    line: decl.row.line,
                    container: None,
                    deprecated: None,
                });
                namespace_rows.push((ns, row));
                row
            });
            let own = index.symbols.len() as u32;
            let line = decl.row.line;
            index.symbols.push(WorkspaceSymbolRow {
                container,
                ..decl.row
            });
            let mut children = decl.children;
            children.sort_by_key(|(key, _)| *key);
            for (_, mut child) in children {
                // Constants and enum cases report their container's line.
                if matches!(child.kind, SymbolKind::CONSTANT | SymbolKind::ENUM_MEMBER) {
                    child.line = line;
                }
                index.symbols.push(WorkspaceSymbolRow {
                    container: Some(own),
                    ..child
                });
            }
            match decl.item {
                DeclItem::Function(f) => index.functions.push(f),
                DeclItem::Class(c) => index.classes.push(*c),
            }
        }
        index
    }

    /// Collects `source` with a throwaway mir database, for callers without a session.
    pub fn from_source(path: &str, source: &str) -> Self {
        let mut db = mir_analyzer::db::MirDbStorage::default();
        let file = db.upsert_source_file(Arc::from(path), Arc::from(source));
        let defs = mir_analyzer::db::collect_file_definitions_uncached(&db, file);
        Self::from_definitions(&defs.slice, source)
    }
}

type SortKey = (u32, u16);
type ChildRow = (SortKey, WorkspaceSymbolRow);

struct Decl {
    key: SortKey,
    namespace: Option<Box<str>>,
    row: WorkspaceSymbolRow,
    children: Vec<ChildRow>,
    item: DeclItem,
}

enum DeclItem {
    Function(FunctionDef),
    Class(Box<ClassDef>),
}

fn sort_key(loc: Option<&Location>) -> SortKey {
    loc.map_or((u32::MAX, u16::MAX), |l| (l.line, l.col_start))
}

fn short_name(fqn: &str) -> &str {
    fqn.rsplit('\\').next().unwrap_or(fqn)
}

fn namespace_of(fqn: &str) -> Option<Box<str>> {
    fqn.trim_start_matches('\\')
        .rsplit_once('\\')
        .map(|(ns, _)| ns.into())
}

/// mir resolves names; a leading `\` keeps them fully-qualified for consumers that re-resolve.
fn absolute(name: &Arc<str>) -> Arc<str> {
    if name.starts_with('\\') {
        Arc::clone(name)
    } else {
        Arc::from(format!("\\{name}"))
    }
}

fn type_string(ty: Option<&Arc<Type>>) -> Option<Box<str>> {
    ty.map(|t| t.to_string().into())
}

fn deprecated_flag(deprecated: &Option<Arc<str>>) -> Option<bool> {
    deprecated.as_ref().map(|_| true)
}

fn visibility(v: mir::Visibility) -> Visibility {
    match v {
        mir::Visibility::Public => Visibility::Public,
        mir::Visibility::Protected => Visibility::Protected,
        mir::Visibility::Private => Visibility::Private,
    }
}

fn param_defs(params: &[mir::DeclaredParam]) -> Vec<ParamDef> {
    params
        .iter()
        .map(|p| ParamDef {
            name: p.name.as_str().trim_start_matches('$').into(),
            type_hint: type_string(p.ty.as_ref()),
            has_default: p.has_default,
            variadic: p.is_variadic,
        })
        .collect()
}

fn row(name: &str, kind: SymbolKind, line: u32, deprecated: Option<bool>) -> WorkspaceSymbolRow {
    WorkspaceSymbolRow {
        name: name.into(),
        kind,
        line,
        container: None,
        deprecated,
    }
}

fn function_decl(text: &SourceText<'_>, f: &mir::FunctionDef) -> Decl {
    let (line, name_char) = text.declared_name(f.location.as_ref(), &["function"], &f.short_name);
    Decl {
        key: sort_key(f.location.as_ref()),
        namespace: namespace_of(&f.fqn),
        row: row(
            &f.short_name,
            SymbolKind::FUNCTION,
            line,
            deprecated_flag(&f.deprecated),
        ),
        children: Vec::new(),
        item: DeclItem::Function(FunctionDef {
            name: f.short_name.as_ref().into(),
            fqn: f.fqn.trim_start_matches('\\').into(),
            params: param_defs(&f.params),
            return_type: type_string(f.return_type.as_ref()),
            start_line: line,
            name_char,
        }),
    }
}

/// Real (non-`@method`) methods with their workspace-symbol rows.
fn methods<'a>(
    text: &SourceText<'_>,
    own: impl Iterator<Item = &'a Arc<mir::MethodDef>>,
    interface: bool,
    rows: &mut Vec<ChildRow>,
) -> Vec<MethodDef> {
    own.filter(|m| !m.is_virtual)
        .map(|m| {
            let (line, name_char) = text.declared_name(m.location.as_ref(), &["function"], &m.name);
            rows.push((
                sort_key(m.location.as_ref()),
                row(
                    &m.name,
                    SymbolKind::METHOD,
                    line,
                    deprecated_flag(&m.deprecated),
                ),
            ));
            MethodDef {
                name: m.name.as_ref().into(),
                is_static: m.is_static,
                is_abstract: interface || m.is_abstract,
                visibility: if interface {
                    Visibility::Public
                } else {
                    visibility(m.visibility)
                },
                params: param_defs(&m.params),
                return_type: type_string(m.return_type.as_ref()),
                start_line: line,
                name_char,
            }
        })
        .collect()
}

fn properties<'a>(
    text: &SourceText<'_>,
    own: impl Iterator<Item = &'a mir::PropertyDef>,
    rows: Option<&mut Vec<ChildRow>>,
) -> Vec<PropertyDef> {
    let mut props: Vec<(SortKey, PropertyDef, Option<bool>)> = own
        .filter(|p| !p.from_docblock)
        .map(|p| {
            let (line, name_char) = text.declared_variable(p.location.as_ref(), &p.name);
            let def = PropertyDef {
                name: p.name.as_ref().into(),
                is_static: p.is_static,
                is_readonly: p.is_readonly,
                type_hint: type_string(p.ty.as_ref()),
                visibility: visibility(p.visibility),
                start_line: line,
                name_char,
            };
            (
                sort_key(p.location.as_ref()),
                def,
                deprecated_flag(&p.deprecated),
            )
        })
        .collect();
    props.sort_by_key(|(key, _, _)| *key);
    if let Some(rows) = rows {
        for (key, p, deprecated) in &props {
            let name = format!("${}", p.name);
            rows.push((
                *key,
                row(&name, SymbolKind::PROPERTY, p.start_line, *deprecated),
            ));
        }
    }
    props.into_iter().map(|(_, p, _)| p).collect()
}

fn constants<'a>(
    own: impl Iterator<Item = &'a mir::ConstantDef>,
    rows: &mut Vec<ChildRow>,
) -> Vec<Box<str>> {
    let mut consts: Vec<&mir::ConstantDef> = own.collect();
    consts.sort_by_key(|c| sort_key(c.location.as_ref()));
    for c in &consts {
        rows.push((
            sort_key(c.location.as_ref()),
            row(
                &c.name,
                SymbolKind::CONSTANT,
                0,
                deprecated_flag(&c.deprecated),
            ),
        ));
    }
    consts.iter().map(|c| c.name.as_ref().into()).collect()
}

fn doc_methods(c: &mir::ClassDef) -> Vec<DocMethodEntry> {
    let mut entries: Vec<(SortKey, DocMethodEntry)> = c
        .own_methods
        .values()
        .filter(|m| m.is_virtual)
        .map(|m| {
            let entry = DocMethodEntry {
                name: m.name.as_ref().into(),
                is_static: m.is_static,
                return_type: type_string(m.return_type.as_ref()),
                start_line: m.location.as_ref().map_or(0, |l| l.line.saturating_sub(1)),
                params: m
                    .params
                    .iter()
                    .map(|p| DocMethodParam {
                        name: p.name.as_str().trim_start_matches('$').into(),
                        type_hint: type_string(p.ty.as_ref()),
                        is_variadic: p.is_variadic,
                        is_byref: p.is_byref,
                        is_optional: p.is_optional,
                    })
                    .collect(),
            };
            (sort_key(m.location.as_ref()), entry)
        })
        .collect();
    entries.sort_by_key(|(key, _)| *key);
    entries.into_iter().map(|(_, e)| e).collect()
}

fn trait_method_aliases(c: &mir::ClassDef) -> Vec<TraitMethodAlias> {
    let mut aliases: Vec<(SortKey, TraitMethodAlias)> = c
        .trait_aliases
        .iter()
        .map(|(key, (trait_name, original, _, alias))| {
            let alias = TraitMethodAlias {
                alias: alias.as_ref().into(),
                original: original.as_ref().into(),
                trait_name: trait_name.as_ref().map(|t| absolute(t).as_ref().into()),
            };
            (sort_key(c.trait_alias_locations.get(key)), alias)
        })
        .collect();
    aliases.sort_by_key(|(key, _)| *key);
    aliases.into_iter().map(|(_, a)| a).collect()
}

struct ClassShell<'a> {
    fqcn: &'a str,
    short_name: &'a str,
    location: Option<&'a Location>,
    deprecated: &'a Option<Arc<str>>,
    kind: ClassKind,
    row_kind: SymbolKind,
}

fn class_like_decl(
    text: &SourceText<'_>,
    shell: ClassShell<'_>,
    children: Vec<ChildRow>,
    build: impl FnOnce(u32, u32) -> ClassDef,
) -> Decl {
    let (line, name_char) = text.declared_name(
        shell.location,
        &["class", "interface", "trait", "enum"],
        shell.short_name,
    );
    let mut def = build(line, name_char);
    def.name = shell.short_name.into();
    def.fqn = shell.fqcn.trim_start_matches('\\').into();
    def.kind = shell.kind;
    Decl {
        key: sort_key(shell.location),
        namespace: namespace_of(shell.fqcn),
        row: row(
            shell.short_name,
            shell.row_kind,
            line,
            deprecated_flag(shell.deprecated),
        ),
        children,
        item: DeclItem::Class(Box::new(def)),
    }
}

fn empty_class(start_line: u32, name_char: u32) -> ClassDef {
    ClassDef {
        name: Box::default(),
        fqn: Box::default(),
        kind: ClassKind::Class,
        is_abstract: false,
        is_readonly: false,
        parent: None,
        implements: Vec::new(),
        traits: Vec::new(),
        methods: Vec::new(),
        properties: Vec::new(),
        constants: Vec::new(),
        cases: Vec::new(),
        start_line,
        name_char,
        doc_methods: Vec::new(),
        mixins: Vec::new(),
        trait_method_aliases: Vec::new(),
    }
}

fn class_decl(text: &SourceText<'_>, c: &mir::ClassDef) -> Decl {
    let mut rows = Vec::new();
    let methods = methods(text, c.own_methods.values(), false, &mut rows);
    let properties = properties(text, c.own_properties.values(), Some(&mut rows));
    let constants = constants(c.own_constants.values(), &mut rows);
    let shell = ClassShell {
        fqcn: &c.fqcn,
        short_name: &c.short_name,
        location: c.location.as_ref(),
        deprecated: &c.deprecated,
        kind: ClassKind::Class,
        row_kind: SymbolKind::CLASS,
    };
    class_like_decl(text, shell, rows, |line, name_char| ClassDef {
        is_abstract: c.is_abstract,
        is_readonly: c.is_readonly,
        parent: c.parent.as_ref().map(absolute),
        implements: c.interfaces.iter().map(absolute).collect(),
        traits: c.traits.iter().map(absolute).collect(),
        methods,
        properties,
        constants,
        doc_methods: doc_methods(c),
        mixins: c.mixins.iter().map(absolute).collect(),
        trait_method_aliases: trait_method_aliases(c),
        ..empty_class(line, name_char)
    })
}

fn interface_decl(text: &SourceText<'_>, i: &mir::InterfaceDef) -> Decl {
    let mut rows = Vec::new();
    let methods = methods(text, i.own_methods.values(), true, &mut rows);
    let constants = constants(i.own_constants.values(), &mut rows);
    let shell = ClassShell {
        fqcn: &i.fqcn,
        short_name: &i.short_name,
        location: i.location.as_ref(),
        deprecated: &i.deprecated,
        kind: ClassKind::Interface,
        row_kind: SymbolKind::INTERFACE,
    };
    class_like_decl(text, shell, rows, |line, name_char| ClassDef {
        is_abstract: true,
        implements: i.extends.iter().map(absolute).collect(),
        methods,
        constants,
        ..empty_class(line, name_char)
    })
}

fn trait_decl(text: &SourceText<'_>, t: &mir::TraitDef) -> Decl {
    let mut rows = Vec::new();
    let methods = methods(text, t.own_methods.values(), false, &mut rows);
    let properties = properties(text, t.own_properties.values(), None);
    let mut constant_rows = Vec::new();
    let constants = constants(t.own_constants.values(), &mut constant_rows);
    let shell = ClassShell {
        fqcn: &t.fqcn,
        short_name: &t.short_name,
        location: t.location.as_ref(),
        deprecated: &t.deprecated,
        kind: ClassKind::Trait,
        row_kind: SymbolKind::CLASS,
    };
    class_like_decl(text, shell, rows, |line, name_char| ClassDef {
        traits: t.traits.iter().map(absolute).collect(),
        methods,
        properties,
        constants,
        ..empty_class(line, name_char)
    })
}

fn enum_decl(text: &SourceText<'_>, e: &mir::EnumDef) -> Decl {
    let mut rows = Vec::new();
    let mut cases: Vec<&mir::EnumCaseDef> = e.cases.values().collect();
    cases.sort_by_key(|c| sort_key(c.location.as_ref()));
    for case in &cases {
        rows.push((
            sort_key(case.location.as_ref()),
            row(&case.name, SymbolKind::ENUM_MEMBER, 0, None),
        ));
    }
    let methods = methods(text, e.own_methods.values(), false, &mut rows);
    let constants = constants(e.own_constants.values(), &mut rows);
    let shell = ClassShell {
        fqcn: &e.fqcn,
        short_name: &e.short_name,
        location: e.location.as_ref(),
        deprecated: &e.deprecated,
        kind: ClassKind::Enum,
        row_kind: SymbolKind::ENUM,
    };
    class_like_decl(text, shell, rows, |line, name_char| ClassDef {
        implements: e.interfaces.iter().map(absolute).collect(),
        traits: e.traits.iter().map(absolute).collect(),
        methods,
        constants,
        cases: cases.iter().map(|c| c.name.as_ref().into()).collect(),
        ..empty_class(line, name_char)
    })
}

/// Source text with a line table, for turning mir's declaration spans into name positions.
struct SourceText<'a> {
    src: &'a str,
    line_starts: Vec<usize>,
}

impl<'a> SourceText<'a> {
    fn new(src: &'a str) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        Self { src, line_starts }
    }

    /// Byte offset of a 1-based line and code-point column.
    fn offset(&self, line: u32, column: u16) -> usize {
        let Some(&start) = self.line_starts.get(line.saturating_sub(1) as usize) else {
            return self.src.len();
        };
        self.src[start..]
            .char_indices()
            .nth(column as usize)
            .map_or(self.src.len(), |(i, _)| start + i)
    }

    fn line_start(&self, line: u32) -> usize {
        self.line_starts
            .get(line.saturating_sub(1) as usize)
            .copied()
            .unwrap_or(self.src.len())
    }

    /// 0-based line and UTF-16 column of a byte offset.
    fn position(&self, offset: usize) -> (u32, u32) {
        let line = self
            .line_starts
            .partition_point(|&s| s <= offset)
            .saturating_sub(1);
        let start = self.line_starts[line];
        let character = self.src[start..offset]
            .chars()
            .map(|c| c.len_utf16() as u32)
            .sum();
        (line as u32, character)
    }

    /// Position of `name` after the first of `keywords` in the declaration span.
    fn declared_name(&self, loc: Option<&Location>, keywords: &[&str], name: &str) -> (u32, u32) {
        self.find_in_span(loc, |src, from, to| {
            let after = keywords
                .iter()
                .filter_map(|k| find_word(src, from, to, k, true).map(|i| i + k.len()))
                .min()?;
            find_word(src, after, to, name, false).or_else(|| find_word(src, after, to, name, true))
        })
    }

    /// Position of `$name`'s identifier in the declaration span.
    fn declared_variable(&self, loc: Option<&Location>, name: &str) -> (u32, u32) {
        let var = format!("${name}");
        self.find_in_span(loc, |src, from, to| {
            find_word(src, from, to, &var, false).map(|i| i + 1)
        })
    }

    fn find_in_span(
        &self,
        loc: Option<&Location>,
        find: impl Fn(&str, usize, usize) -> Option<usize>,
    ) -> (u32, u32) {
        let Some(loc) = loc else { return (0, 0) };
        let start = self.offset(loc.line, loc.col_start);
        let end = self.offset(loc.line_end, loc.col_end).max(start);
        // Columns are u16; on longer lines retry from the start of the line.
        let found =
            find(self.src, start, end).or_else(|| find(self.src, self.line_start(loc.line), end));
        match found {
            Some(offset) => self.position(offset),
            None => self.position(start.min(self.src.len())),
        }
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80
}

/// First whole-identifier occurrence of `word` in `src[from..to]`.
fn find_word(src: &str, from: usize, to: usize, word: &str, ignore_case: bool) -> Option<usize> {
    let bytes = src.as_bytes();
    let w = word.as_bytes();
    let to = to.min(bytes.len());
    if w.is_empty() || from >= to {
        return None;
    }
    let first_is_ident = is_ident_byte(w[0]);
    (from..=to.checked_sub(w.len())?).find(|&i| {
        let candidate = &bytes[i..i + w.len()];
        let matches = if ignore_case {
            candidate.eq_ignore_ascii_case(w)
        } else {
            candidate == w
        };
        matches
            && (!first_is_ident || i == 0 || !is_ident_byte(bytes[i - 1]))
            && bytes.get(i + w.len()).is_none_or(|&b| !is_ident_byte(b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_class_and_method() {
        let src = "<?php\nclass Greeter {\n    public function greet(string $name): string {}\n}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert_eq!(idx.classes.len(), 1);
        let cls = &idx.classes[0];
        assert_eq!(&*cls.name, "Greeter");
        assert_eq!(cls.kind, ClassKind::Class);
        assert_eq!(cls.start_line, 1);
        assert_eq!(cls.methods.len(), 1);
        let method = &cls.methods[0];
        assert_eq!(&*method.name, "greet");
        assert_eq!(method.return_type.as_deref(), Some("string"));
        assert_eq!(method.params.len(), 1);
        assert_eq!(&*method.params[0].name, "name");
        assert_eq!(method.params[0].type_hint.as_deref(), Some("string"));
    }

    #[test]
    fn extracts_function() {
        let src = "<?php\nfunction add(int $a, int $b): int {}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert_eq!(idx.functions.len(), 1);
        let f = &idx.functions[0];
        assert_eq!(&*f.name, "add");
        assert_eq!(f.return_type.as_deref(), Some("int"));
        assert_eq!(f.params.len(), 2);
        assert_eq!(&*f.params[0].name, "a");
        assert_eq!(f.params[0].type_hint.as_deref(), Some("int"));
        assert_eq!(&*f.params[1].name, "b");
        assert_eq!(f.params[1].type_hint.as_deref(), Some("int"));
    }

    #[test]
    fn extracts_namespace() {
        let src = "<?php\nnamespace App\\Services;\nclass Mailer {}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert_eq!(idx.namespace.as_deref(), Some("App\\Services"));
        assert_eq!(&*idx.classes[0].fqn, "App\\Services\\Mailer");
    }

    #[test]
    fn extracts_braced_namespace() {
        let src = "<?php\nnamespace App\\Models {\n    class User {}\n}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert_eq!(idx.namespace.as_deref(), Some("App\\Models"));
        assert_eq!(&*idx.classes[0].fqn, "App\\Models\\User");
    }

    #[test]
    fn extracts_interface() {
        let src = "<?php\ninterface Countable {\n    public function count(): int;\n}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert_eq!(idx.classes.len(), 1);
        assert_eq!(idx.classes[0].kind, ClassKind::Interface);
        assert_eq!(&*idx.classes[0].methods[0].name, "count");
        assert!(idx.classes[0].methods[0].is_abstract);
    }

    #[test]
    fn extracts_trait() {
        let src = "<?php\ntrait Loggable {\n    public function log(): void {}\n}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert_eq!(idx.classes[0].kind, ClassKind::Trait);
        assert_eq!(&*idx.classes[0].methods[0].name, "log");
    }

    #[test]
    fn extracts_enum_cases() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert_eq!(idx.classes[0].kind, ClassKind::Enum);
        assert!(idx.classes[0].cases.iter().any(|c| c.as_ref() == "Active"));
        assert!(
            idx.classes[0]
                .cases
                .iter()
                .any(|c| c.as_ref() == "Inactive")
        );
    }

    #[test]
    fn readonly_class_is_marked() {
        let src = "<?php\nreadonly class ValueObject { public function __construct(public string $id) {} }";
        let idx = FileIndex::from_source("file:///t.php", src);
        let cls = &idx.classes[0];
        assert!(cls.is_readonly);
    }

    #[test]
    fn non_readonly_class_is_not_marked() {
        let src = "<?php\nclass Plain {}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert!(!idx.classes[0].is_readonly);
    }

    #[test]
    fn readonly_property_is_marked() {
        let src = "<?php\nclass Dto { public readonly string $value; }";
        let idx = FileIndex::from_source("file:///t.php", src);
        let prop = &idx.classes[0].properties[0];
        assert!(prop.is_readonly);
    }

    #[test]
    fn non_readonly_property_is_not_marked() {
        let src = "<?php\nclass Dto { public string $value; }";
        let idx = FileIndex::from_source("file:///t.php", src);
        let prop = &idx.classes[0].properties[0];
        assert!(!prop.is_readonly);
    }

    #[test]
    fn readonly_promoted_param_is_marked() {
        let src = "<?php\nclass Dto { public function __construct(public readonly string $id) {} }";
        let idx = FileIndex::from_source("file:///t.php", src);
        let prop = idx.classes[0]
            .properties
            .iter()
            .find(|p| p.name.as_ref() == "id")
            .expect("promoted property 'id' should be indexed");
        assert!(prop.is_readonly);
    }

    #[test]
    fn extracts_class_properties_and_constants() {
        let src = "<?php\nclass Config {\n    public string $host;\n    const VERSION = '1.0';\n}";
        let idx = FileIndex::from_source("file:///t.php", src);
        let cls = &idx.classes[0];
        assert_eq!(cls.properties.len(), 1);
        assert_eq!(&*cls.properties[0].name, "host");
        assert!(cls.constants.iter().any(|c| c.as_ref() == "VERSION"));
    }

    #[test]
    fn extracts_trait_use() {
        let src = "<?php\ntrait T {}\nclass MyClass { use T; }";
        let idx = FileIndex::from_source("file:///t.php", src);
        let cls = idx
            .classes
            .iter()
            .find(|c| c.name.as_ref() == "MyClass")
            .unwrap();
        assert!(cls.traits.iter().any(|t| t.as_ref() == "\\T"));
    }

    #[test]
    fn extracts_class_implements_and_extends() {
        let src = "<?php\nclass Dog extends Animal implements Pet, Movable {}";
        let idx = FileIndex::from_source("file:///t.php", src);
        let cls = &idx.classes[0];
        assert_eq!(cls.parent.as_deref(), Some("\\Animal"));
        assert!(cls.implements.iter().any(|i| i.as_ref() == "\\Pet"));
        assert!(cls.implements.iter().any(|i| i.as_ref() == "\\Movable"));
    }

    #[test]
    fn constructor_promoted_params_become_properties() {
        let src = "<?php\nclass User {\n    public function __construct(public string $name) {}\n}";
        let idx = FileIndex::from_source("file:///t.php", src);
        let cls = &idx.classes[0];
        // Should have a property from the promoted param.
        assert!(
            cls.properties.iter().any(|p| p.name.as_ref() == "name"),
            "expected promoted property 'name', got: {:?}",
            cls.properties.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn may_declare_matches_every_indexed_declaration_kind() {
        let index = FileIndex::from_source(
            "file:///a.php",
            "<?php
            function helper() {}
            /** @method tagged() */
            class Owner {
                const LIMIT = 1;
                public int $count = 0;
                function run() {}
            }",
        );
        for word in ["helper", "Owner", "LIMIT", "run", "tagged", "TAGGED"] {
            assert!(index.may_declare(word, word), "{word}");
        }
        assert!(index.may_declare("$count", "count"));
        assert!(!index.may_declare("unrelated", "unrelated"));
        assert!(!index.may_declare("count", "unrelated"));
    }

    #[test]
    fn may_declare_keeps_members_the_index_omits_as_candidates() {
        for source in [
            "<?php /** @method tagged() */ interface Shape {}",
            "<?php /** @method tagged() */ trait Shape {}",
            "<?php /** @method tagged() */ enum Shape {}",
        ] {
            let index = FileIndex::from_source("file:///a.php", source);
            assert!(index.may_declare("tagged", "tagged"), "{source}");
        }
    }

    #[test]
    fn extracts_doc_methods_from_class_docblock() {
        let src = "<?php\n/**\n * @method User find(int $id)\n * @method static Builder where(string $col, mixed $val)\n */\nclass Model {}";
        let idx = FileIndex::from_source("file:///t.php", src);
        let cls = &idx.classes[0];
        assert_eq!(cls.doc_methods.len(), 2, "expected 2 @method entries");

        let find = cls.doc_methods.iter().find(|m| m.name.as_ref() == "find");
        assert!(find.is_some(), "expected @method find");
        let find = find.unwrap();
        assert!(!find.is_static);
        assert_eq!(find.return_type.as_deref(), Some("User"));
        assert_eq!(find.start_line, 2); // 0-based: line 0=<?php, 1=/**, 2=@method find

        let where_m = cls.doc_methods.iter().find(|m| m.name.as_ref() == "where");
        assert!(where_m.is_some(), "expected @method where");
        let where_m = where_m.unwrap();
        assert!(where_m.is_static);
        assert_eq!(where_m.return_type.as_deref(), Some("Builder"));
        assert_eq!(where_m.start_line, 3); // 0-based: line 3=@method static where
    }

    #[test]
    fn doc_method_tag_line_no_substring_collision() {
        // `log` has a param named `$find`; `find` must resolve to its own line, not `log`'s.
        let src = "<?php\n/**\n * @method void log(string $find)\n * @method Model find()\n */\nclass Builder {}";
        let idx = FileIndex::from_source("file:///t.php", src);
        let cls = &idx.classes[0];
        let find = cls.doc_methods.iter().find(|m| m.name.as_ref() == "find");
        assert!(find.is_some(), "expected @method find");
        assert_eq!(find.unwrap().start_line, 3); // line 3 = `@method Model find()`, not line 2
    }

    #[test]
    fn class_without_docblock_has_no_doc_methods() {
        let src = "<?php\nclass Plain {\n    public function foo(): void {}\n}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert!(idx.classes[0].doc_methods.is_empty());
    }

    #[test]
    fn extracts_mixins_from_class_docblock() {
        let src = "<?php\n/**\n * @mixin Macroable\n * @mixin HasEvents\n */\nclass Builder {}";
        let idx = FileIndex::from_source("file:///t.php", src);
        let cls = &idx.classes[0];
        assert_eq!(cls.mixins.len(), 2, "expected 2 @mixin entries");
        assert!(cls.mixins.iter().any(|m| m.as_ref() == "\\Macroable"));
        assert!(cls.mixins.iter().any(|m| m.as_ref() == "\\HasEvents"));
    }

    #[test]
    fn class_without_docblock_has_no_mixins() {
        let src = "<?php\nclass Plain {}";
        let idx = FileIndex::from_source("file:///t.php", src);
        assert!(idx.classes[0].mixins.is_empty());
    }
}
