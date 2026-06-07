/// Compact symbol index extracted from a parsed PHP file.
///
/// A `FileIndex` captures only the declaration-level information needed for
/// cross-file features (go-to-definition, workspace symbols, hover signatures,
/// find-implementations, etc.).  It is ~2 KB per file compared to ~100 KB for
/// a full `ParsedDoc`, allowing the LSP to keep thousands of background files
/// in memory without exhausting RAM.
///
/// Call [`FileIndex::extract`] right after parsing; the `ParsedDoc` (and its
/// bumpalo arena) can be dropped immediately after extraction.
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};

use crate::ast::{ParsedDoc, format_type_hint};
use crate::docblock::parse_docblock;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FileIndex {
    pub namespace: Option<Box<str>>,
    pub functions: Vec<FunctionDef>,
    pub classes: Vec<ClassDef>,
    pub constants: Vec<Box<str>>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FunctionDef {
    pub name: Box<str>,
    /// Fully-qualified name: `\Namespace\function_name` or just `function_name`.
    pub fqn: Box<str>,
    pub params: Vec<ParamDef>,
    pub return_type: Option<Box<str>>,
    /// Raw docblock text (the `/** … */` comment before the declaration).
    pub doc: Option<Box<str>>,
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
    pub doc: Option<Box<str>>,
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
    pub type_hint: Option<Box<str>>,
    pub visibility: Visibility,
    pub start_line: u32,
    /// Character position of the property name on its line (UTF-16 code units).
    pub name_char: u32,
}

// ── Extract ───────────────────────────────────────────────────────────────────

impl FileIndex {
    /// Walk `doc.program().stmts` once and build a compact symbol index.
    pub fn extract(doc: &ParsedDoc) -> Self {
        let source = doc.source();
        let view = doc.view();
        let mut index = FileIndex::default();
        collect_stmts(source, &view, &doc.program().stmts, None, &mut index);
        index
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────────

fn fqn(namespace: Option<&str>, name: &str) -> Box<str> {
    match namespace {
        Some(ns) if !ns.is_empty() => format!("{}\\{}", ns, name).into(),
        _ => name.into(),
    }
}

fn collect_stmts(
    source: &str,
    view: &crate::ast::SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    namespace: Option<&str>,
    index: &mut FileIndex,
) {
    use crate::ast::str_offset;

    let name_char = |name: &str| -> u32 {
        str_offset(source, name)
            .map(|off| view.position_of(off).character)
            .unwrap_or(0)
    };

    // Track the current namespace for unbraced `namespace Foo;` statements.
    let mut cur_ns: Option<Box<str>> = namespace.map(|s| s.into());

    for stmt in stmts {
        match &stmt.kind {
            // ── Namespace ────────────────────────────────────────────────────
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().into());

                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        // Braced namespace: recurse with its name as context.
                        let ns_str = ns_name.as_deref();
                        // Update the top-level namespace if not already set.
                        if index.namespace.is_none() {
                            index.namespace = ns_name.clone();
                        }
                        collect_stmts(source, view, &inner.stmts, ns_str, index);
                    }
                    NamespaceBody::Simple => {
                        // Unbraced namespace: all following stmts belong to it.
                        if index.namespace.is_none() {
                            index.namespace = ns_name.clone();
                        }
                        cur_ns = ns_name;
                    }
                }
            }

            // ── Top-level function ───────────────────────────────────────────
            StmtKind::Function(f) => {
                let doc_text = f.doc_comment.as_ref().map(|c| c.text.into());
                let start_line = view.position_of(stmt.span.start).line;
                let ns = cur_ns.as_deref();
                let f_name = f.name.or_error();
                index.functions.push(FunctionDef {
                    name: Box::from(f_name),
                    fqn: fqn(ns, f_name),
                    params: extract_params(&f.params),
                    return_type: f.return_type.as_ref().map(|t| format_type_hint(t).into()),
                    doc: doc_text,
                    start_line,
                    name_char: name_char(f_name),
                });
            }

            // ── Class ────────────────────────────────────────────────────────
            StmtKind::Class(c) => {
                let Some(class_name) = c.name else { continue };
                let class_name_str = class_name.or_error();
                let start_line = view.position_of(stmt.span.start).line;
                let ns = cur_ns.as_deref();

                let mut class_def = ClassDef {
                    name: Box::from(class_name_str),
                    fqn: fqn(ns, class_name_str),
                    kind: ClassKind::Class,
                    is_abstract: c.modifiers.is_abstract,
                    parent: c
                        .extends
                        .as_ref()
                        .map(|e| Arc::from(e.to_string_repr().as_ref())),
                    implements: c
                        .implements
                        .iter()
                        .map(|i| Arc::from(i.to_string_repr().as_ref()))
                        .collect(),
                    traits: Vec::new(),
                    methods: Vec::new(),
                    properties: Vec::new(),
                    constants: Vec::new(),
                    cases: Vec::new(),
                    start_line,
                    name_char: name_char(class_name_str),
                    doc_methods: Vec::new(),
                };

                for member in c.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mdoc = m.doc_comment.as_ref().map(|c| c.text.into());
                            let mstart = view.position_of(member.span.start).line;
                            let vis = method_visibility(m.visibility);
                            let method_params = extract_params(&m.params);
                            // Constructor-promoted params → also add as PropertyDef.
                            for ast_param in m.params.iter() {
                                if ast_param.visibility.is_some() {
                                    let pvis = method_visibility(ast_param.visibility);
                                    let pstart = view.position_of(ast_param.span.start).line;
                                    let p_name = ast_param.name.or_error();
                                    class_def.properties.push(PropertyDef {
                                        name: Box::from(p_name),
                                        is_static: false,
                                        type_hint: ast_param
                                            .type_hint
                                            .as_ref()
                                            .map(|t| format_type_hint(t).into()),
                                        visibility: pvis,
                                        start_line: pstart,
                                        name_char: name_char(p_name),
                                    });
                                }
                            }
                            let m_name = m.name.or_error();
                            class_def.methods.push(MethodDef {
                                name: Box::from(m_name),
                                is_static: m.is_static,
                                is_abstract: m.is_abstract,
                                visibility: vis,
                                params: method_params,
                                return_type: m
                                    .return_type
                                    .as_ref()
                                    .map(|t| format_type_hint(t).into()),
                                doc: mdoc,
                                start_line: mstart,
                                name_char: name_char(m_name),
                            });
                        }
                        ClassMemberKind::Property(p) => {
                            let vis = method_visibility(p.visibility);
                            let pstart = view.position_of(member.span.start).line;
                            let p_name = p.name.or_error();
                            class_def.properties.push(PropertyDef {
                                name: Box::from(p_name),
                                is_static: p.is_static,
                                type_hint: p.type_hint.as_ref().map(|t| format_type_hint(t).into()),
                                visibility: vis,
                                start_line: pstart,
                                name_char: name_char(p_name),
                            });
                        }
                        ClassMemberKind::ClassConst(cc) => {
                            class_def.constants.push(Box::from(cc.name.or_error()));
                        }
                        ClassMemberKind::TraitUse(tu) => {
                            for t in tu.traits.iter() {
                                class_def
                                    .traits
                                    .push(Arc::from(t.to_string_repr().as_ref()));
                            }
                        }
                    }
                }
                // Extract `@method` docblock tags as virtual method entries so
                // go-to-definition can navigate to the docblock line.
                if let Some(doc) = &c.doc_comment {
                    let db = parse_docblock(doc.text);
                    for dm in &db.methods {
                        let line = doc_method_tag_line(view, doc, &dm.name);
                        let ret = if dm.return_type.is_empty() {
                            None
                        } else {
                            Some(Box::from(dm.return_type.as_str()))
                        };
                        class_def.doc_methods.push(DocMethodEntry {
                            name: Box::from(dm.name.as_str()),
                            is_static: dm.is_static,
                            return_type: ret,
                            start_line: line,
                        });
                    }
                }
                index.classes.push(class_def);
            }

            // ── Interface ────────────────────────────────────────────────────
            StmtKind::Interface(i) => {
                let start_line = view.position_of(stmt.span.start).line;
                let ns = cur_ns.as_deref();

                let i_name = i.name.or_error();
                let mut iface_def = ClassDef {
                    name: Box::from(i_name),
                    fqn: fqn(ns, i_name),
                    kind: ClassKind::Interface,
                    is_abstract: true,
                    parent: None,
                    implements: i
                        .extends
                        .iter()
                        .map(|e| Arc::from(e.to_string_repr().as_ref()))
                        .collect(),
                    traits: Vec::new(),
                    methods: Vec::new(),
                    properties: Vec::new(),
                    constants: Vec::new(),
                    cases: Vec::new(),
                    start_line,
                    name_char: name_char(i_name),
                    doc_methods: Vec::new(),
                };

                for member in i.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mdoc = m.doc_comment.as_ref().map(|c| c.text.into());
                            let mstart = view.position_of(member.span.start).line;
                            let m_name = m.name.or_error();
                            iface_def.methods.push(MethodDef {
                                name: Box::from(m_name),
                                is_static: m.is_static,
                                is_abstract: true,
                                visibility: Visibility::Public,
                                params: extract_params(&m.params),
                                return_type: m
                                    .return_type
                                    .as_ref()
                                    .map(|t| format_type_hint(t).into()),
                                doc: mdoc,
                                start_line: mstart,
                                name_char: name_char(m_name),
                            });
                        }
                        ClassMemberKind::ClassConst(cc) => {
                            iface_def.constants.push(Box::from(cc.name.or_error()));
                        }
                        _ => {}
                    }
                }
                index.classes.push(iface_def);
            }

            // ── Trait ────────────────────────────────────────────────────────
            StmtKind::Trait(t) => {
                let start_line = view.position_of(stmt.span.start).line;
                let ns = cur_ns.as_deref();

                let t_name = t.name.or_error();
                let mut trait_def = ClassDef {
                    name: Box::from(t_name),
                    fqn: fqn(ns, t_name),
                    kind: ClassKind::Trait,
                    is_abstract: false,
                    parent: None,
                    implements: Vec::new(),
                    traits: Vec::new(),
                    methods: Vec::new(),
                    properties: Vec::new(),
                    constants: Vec::new(),
                    cases: Vec::new(),
                    start_line,
                    name_char: name_char(t_name),
                    doc_methods: Vec::new(),
                };

                for member in t.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mdoc = m.doc_comment.as_ref().map(|c| c.text.into());
                            let mstart = view.position_of(member.span.start).line;
                            let vis = method_visibility(m.visibility);
                            let m_name = m.name.or_error();
                            trait_def.methods.push(MethodDef {
                                name: Box::from(m_name),
                                is_static: m.is_static,
                                is_abstract: m.is_abstract,
                                visibility: vis,
                                params: extract_params(&m.params),
                                return_type: m
                                    .return_type
                                    .as_ref()
                                    .map(|t| format_type_hint(t).into()),
                                doc: mdoc,
                                start_line: mstart,
                                name_char: name_char(m_name),
                            });
                        }
                        ClassMemberKind::Property(p) => {
                            let vis = method_visibility(p.visibility);
                            let pstart = view.position_of(member.span.start).line;
                            let p_name = p.name.or_error();
                            trait_def.properties.push(PropertyDef {
                                name: Box::from(p_name),
                                is_static: p.is_static,
                                type_hint: p.type_hint.as_ref().map(|t| format_type_hint(t).into()),
                                visibility: vis,
                                start_line: pstart,
                                name_char: name_char(p_name),
                            });
                        }
                        ClassMemberKind::ClassConst(cc) => {
                            trait_def.constants.push(Box::from(cc.name.or_error()));
                        }
                        ClassMemberKind::TraitUse(tu) => {
                            for tr in tu.traits.iter() {
                                trait_def
                                    .traits
                                    .push(Arc::from(tr.to_string_repr().as_ref()));
                            }
                        }
                    }
                }
                index.classes.push(trait_def);
            }

            // ── Enum ─────────────────────────────────────────────────────────
            StmtKind::Enum(e) => {
                let start_line = view.position_of(stmt.span.start).line;
                let ns = cur_ns.as_deref();

                let e_name = e.name.or_error();
                let mut enum_def = ClassDef {
                    name: Box::from(e_name),
                    fqn: fqn(ns, e_name),
                    kind: ClassKind::Enum,
                    is_abstract: false,
                    parent: None,
                    implements: e
                        .implements
                        .iter()
                        .map(|i| Arc::from(i.to_string_repr().as_ref()))
                        .collect(),
                    traits: Vec::new(),
                    methods: Vec::new(),
                    properties: Vec::new(),
                    constants: Vec::new(),
                    cases: Vec::new(),
                    start_line,
                    name_char: name_char(e_name),
                    doc_methods: Vec::new(),
                };

                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Case(c) => {
                            enum_def.cases.push(Box::from(c.name.or_error()));
                        }
                        EnumMemberKind::Method(m) => {
                            let mdoc = m.doc_comment.as_ref().map(|c| c.text.into());
                            let mstart = view.position_of(member.span.start).line;
                            let vis = method_visibility(m.visibility);
                            let m_name = m.name.or_error();
                            enum_def.methods.push(MethodDef {
                                name: Box::from(m_name),
                                is_static: m.is_static,
                                is_abstract: m.is_abstract,
                                visibility: vis,
                                params: extract_params(&m.params),
                                return_type: m
                                    .return_type
                                    .as_ref()
                                    .map(|t| format_type_hint(t).into()),
                                doc: mdoc,
                                start_line: mstart,
                                name_char: name_char(m_name),
                            });
                        }
                        EnumMemberKind::ClassConst(cc) => {
                            enum_def.constants.push(Box::from(cc.name.or_error()));
                        }
                        _ => {}
                    }
                }
                index.classes.push(enum_def);
            }

            // ── Top-level const ──────────────────────────────────────────────
            StmtKind::Const(consts) => {
                for c in consts.iter() {
                    index.constants.push(Box::from(c.name.or_error()));
                }
            }

            _ => {}
        }
    }
}

fn extract_params<'a, 'b>(params: &[php_ast::Param<'a, 'b>]) -> Vec<ParamDef> {
    params
        .iter()
        .map(|p| ParamDef {
            name: Box::from(p.name.or_error()),
            type_hint: p.type_hint.as_ref().map(|t| format_type_hint(t).into()),
            has_default: p.default.is_some(),
            variadic: p.variadic,
        })
        .collect()
}

fn method_visibility(vis: Option<php_ast::Visibility>) -> Visibility {
    match vis {
        Some(php_ast::Visibility::Protected) => Visibility::Protected,
        Some(php_ast::Visibility::Private) => Visibility::Private,
        _ => Visibility::Public,
    }
}

/// Return the source line (0-based) of the `@method method_name` tag within
/// `doc_comment`. Falls back to the docblock's own start line if not found.
fn doc_method_tag_line(
    view: &crate::ast::SourceView<'_>,
    doc_comment: &php_ast::Comment<'_>,
    method_name: &str,
) -> u32 {
    let text = doc_comment.text;
    let base = doc_comment.span.start as usize;
    let mut offset = 0usize;
    while let Some(tag_pos) = text[offset..].find("@method") {
        let segment_start = offset + tag_pos;
        let segment = &text[segment_start..];
        let line_len = segment.find('\n').unwrap_or(segment.len());
        // Require `method_name(` to avoid matching the name as a substring
        // inside a parameter name (e.g. `@method void log(string $find)` must
        // not match when looking for `find`).
        let needle = format!("{}(", method_name);
        if segment[..line_len].contains(needle.as_str()) {
            return view.position_of((base + segment_start) as u32).line;
        }
        offset = segment_start + "@method".len();
    }
    view.position_of(doc_comment.span.start).line
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_class_and_method() {
        let src = "<?php\nclass Greeter {\n    public function greet(string $name): string {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.classes.len(), 1);
        let cls = &idx.classes[0];
        assert_eq!(cls.name, "Greeter".into());
        assert_eq!(cls.kind, ClassKind::Class);
        assert_eq!(cls.start_line, 1);
        assert_eq!(cls.methods.len(), 1);
        let method = &cls.methods[0];
        assert_eq!(method.name, "greet".into());
        assert_eq!(method.return_type.as_deref(), Some("string"));
        assert_eq!(method.params.len(), 1);
        assert_eq!(method.params[0].name, "name".into());
        assert_eq!(method.params[0].type_hint.as_deref(), Some("string"));
    }

    #[test]
    fn extracts_function() {
        let src = "<?php\nfunction add(int $a, int $b): int {}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.functions.len(), 1);
        let f = &idx.functions[0];
        assert_eq!(f.name, "add".into());
        assert_eq!(f.return_type.as_deref(), Some("int"));
        assert_eq!(f.params.len(), 2);
    }

    #[test]
    fn extracts_namespace() {
        let src = "<?php\nnamespace App\\Services;\nclass Mailer {}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.namespace.as_deref(), Some("App\\Services"));
        assert_eq!(idx.classes[0].fqn, "App\\Services\\Mailer".into());
    }

    #[test]
    fn extracts_braced_namespace() {
        let src = "<?php\nnamespace App\\Models {\n    class User {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.namespace.as_deref(), Some("App\\Models"));
        assert_eq!(idx.classes[0].fqn, "App\\Models\\User".into());
    }

    #[test]
    fn extracts_interface() {
        let src = "<?php\ninterface Countable {\n    public function count(): int;\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.classes.len(), 1);
        assert_eq!(idx.classes[0].kind, ClassKind::Interface);
        assert_eq!(idx.classes[0].methods[0].name, "count".into());
        assert!(idx.classes[0].methods[0].is_abstract);
    }

    #[test]
    fn extracts_trait() {
        let src = "<?php\ntrait Loggable {\n    public function log(): void {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.classes[0].kind, ClassKind::Trait);
        assert_eq!(idx.classes[0].methods[0].name, "log".into());
    }

    #[test]
    fn extracts_enum_cases() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
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
    fn extracts_class_properties_and_constants() {
        let src = "<?php\nclass Config {\n    public string $host;\n    const VERSION = '1.0';\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        let cls = &idx.classes[0];
        assert_eq!(cls.properties.len(), 1);
        assert_eq!(cls.properties[0].name, "host".into());
        assert!(cls.constants.iter().any(|c| c.as_ref() == "VERSION"));
    }

    #[test]
    fn extracts_trait_use() {
        let src = "<?php\ntrait T {}\nclass MyClass { use T; }";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        let cls = idx
            .classes
            .iter()
            .find(|c| c.name.as_ref() == "MyClass")
            .unwrap();
        assert!(cls.traits.iter().any(|t| t.as_ref() == "T"));
    }

    #[test]
    fn extracts_class_implements_and_extends() {
        let src = "<?php\nclass Dog extends Animal implements Pet, Movable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        let cls = &idx.classes[0];
        assert_eq!(cls.parent.as_deref(), Some("Animal"));
        assert!(cls.implements.iter().any(|i| i.as_ref() == "Pet"));
        assert!(cls.implements.iter().any(|i| i.as_ref() == "Movable"));
    }

    #[test]
    fn constructor_promoted_params_become_properties() {
        let src = "<?php\nclass User {\n    public function __construct(public string $name) {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        let cls = &idx.classes[0];
        // Should have a property from the promoted param.
        assert!(
            cls.properties.iter().any(|p| p.name.as_ref() == "name"),
            "expected promoted property 'name', got: {:?}",
            cls.properties.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn extracts_doc_methods_from_class_docblock() {
        let src = "<?php\n/**\n * @method User find(int $id)\n * @method static Builder where(string $col, mixed $val)\n */\nclass Model {}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
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
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        let cls = &idx.classes[0];
        let find = cls.doc_methods.iter().find(|m| m.name.as_ref() == "find");
        assert!(find.is_some(), "expected @method find");
        assert_eq!(find.unwrap().start_line, 3); // line 3 = `@method Model find()`, not line 2
    }

    #[test]
    fn class_without_docblock_has_no_doc_methods() {
        let src = "<?php\nclass Plain {\n    public function foo(): void {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert!(idx.classes[0].doc_methods.is_empty());
    }
}
