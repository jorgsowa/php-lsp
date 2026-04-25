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
use crate::docblock::docblock_before;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct FileIndex {
    pub namespace: Option<String>,
    pub functions: Vec<FunctionDef>,
    pub classes: Vec<ClassDef>,
    pub constants: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    /// Fully-qualified name: `\Namespace\function_name` or just `function_name`.
    pub fqn: String,
    pub params: Vec<ParamDef>,
    pub return_type: Option<String>,
    /// Raw docblock text (the `/** … */` comment before the declaration).
    pub doc: Option<String>,
    pub start_line: u32,
}

#[derive(Debug, Clone)]
pub struct ParamDef {
    pub name: String,
    pub type_hint: Option<String>,
    pub has_default: bool,
    pub variadic: bool,
}

#[derive(Debug, Clone)]
pub struct ClassDef {
    pub name: String,
    /// Fully-qualified name.
    pub fqn: String,
    pub kind: ClassKind,
    pub is_abstract: bool,
    /// `extends` clause as written in source (may be short name or FQN).
    pub parent: Option<Arc<str>>,
    pub implements: Vec<Arc<str>>,
    pub traits: Vec<Arc<str>>,
    pub methods: Vec<MethodDef>,
    pub properties: Vec<PropertyDef>,
    pub constants: Vec<String>,
    /// Enum case names (only populated for `ClassKind::Enum`).
    pub cases: Vec<String>,
    pub start_line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
}

#[derive(Debug, Clone)]
pub struct MethodDef {
    pub name: String,
    pub is_static: bool,
    pub is_abstract: bool,
    pub visibility: Visibility,
    pub params: Vec<ParamDef>,
    pub return_type: Option<String>,
    pub doc: Option<String>,
    pub start_line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone)]
pub struct PropertyDef {
    pub name: String,
    pub is_static: bool,
    pub type_hint: Option<String>,
    pub visibility: Visibility,
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

fn fqn(namespace: Option<&str>, name: &str) -> String {
    match namespace {
        Some(ns) if !ns.is_empty() => format!("{}\\{}", ns, name),
        _ => name.to_string(),
    }
}

fn collect_stmts(
    source: &str,
    view: &crate::ast::SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    namespace: Option<&str>,
    index: &mut FileIndex,
) {
    // Track the current namespace for unbraced `namespace Foo;` statements.
    let mut cur_ns: Option<String> = namespace.map(str::to_string);

    for stmt in stmts {
        match &stmt.kind {
            // ── Namespace ────────────────────────────────────────────────────
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().to_string());

                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        // Braced namespace: recurse with its name as context.
                        let ns_str = ns_name.as_deref();
                        // Update the top-level namespace if not already set.
                        if index.namespace.is_none() {
                            index.namespace = ns_name.clone();
                        }
                        collect_stmts(source, view, inner, ns_str, index);
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
                let doc_text = docblock_before(source, stmt.span.start);
                let start_line = view.position_of(stmt.span.start).line;
                let ns = cur_ns.as_deref();
                index.functions.push(FunctionDef {
                    name: f.name.to_string(),
                    fqn: fqn(ns, f.name),
                    params: extract_params(&f.params),
                    return_type: f.return_type.as_ref().map(format_type_hint),
                    doc: doc_text,
                    start_line,
                });
            }

            // ── Class ────────────────────────────────────────────────────────
            StmtKind::Class(c) => {
                let Some(class_name) = c.name else { continue };
                let start_line = view.position_of(stmt.span.start).line;
                let ns = cur_ns.as_deref();

                let mut class_def = ClassDef {
                    name: class_name.to_string(),
                    fqn: fqn(ns, class_name),
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
                };

                for member in c.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mdoc = docblock_before(source, member.span.start);
                            let mstart = view.position_of(member.span.start).line;
                            let vis = method_visibility(m.visibility);
                            let method_params = extract_params(&m.params);
                            // Constructor-promoted params → also add as PropertyDef.
                            for ast_param in m.params.iter() {
                                if ast_param.visibility.is_some() {
                                    let pvis = method_visibility(ast_param.visibility);
                                    class_def.properties.push(PropertyDef {
                                        name: ast_param.name.to_string(),
                                        is_static: false,
                                        type_hint: ast_param
                                            .type_hint
                                            .as_ref()
                                            .map(format_type_hint),
                                        visibility: pvis,
                                    });
                                }
                            }
                            class_def.methods.push(MethodDef {
                                name: m.name.to_string(),
                                is_static: m.is_static,
                                is_abstract: m.is_abstract,
                                visibility: vis,
                                params: method_params,
                                return_type: m.return_type.as_ref().map(format_type_hint),
                                doc: mdoc,
                                start_line: mstart,
                            });
                        }
                        ClassMemberKind::Property(p) => {
                            let vis = method_visibility(p.visibility);
                            class_def.properties.push(PropertyDef {
                                name: p.name.to_string(),
                                is_static: p.is_static,
                                type_hint: p.type_hint.as_ref().map(format_type_hint),
                                visibility: vis,
                            });
                        }
                        ClassMemberKind::ClassConst(cc) => {
                            class_def.constants.push(cc.name.to_string());
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
                index.classes.push(class_def);
            }

            // ── Interface ────────────────────────────────────────────────────
            StmtKind::Interface(i) => {
                let start_line = view.position_of(stmt.span.start).line;
                let ns = cur_ns.as_deref();

                let mut iface_def = ClassDef {
                    name: i.name.to_string(),
                    fqn: fqn(ns, i.name),
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
                };

                for member in i.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mdoc = docblock_before(source, member.span.start);
                            let mstart = view.position_of(member.span.start).line;
                            iface_def.methods.push(MethodDef {
                                name: m.name.to_string(),
                                is_static: m.is_static,
                                is_abstract: true,
                                visibility: Visibility::Public,
                                params: extract_params(&m.params),
                                return_type: m.return_type.as_ref().map(format_type_hint),
                                doc: mdoc,
                                start_line: mstart,
                            });
                        }
                        ClassMemberKind::ClassConst(cc) => {
                            iface_def.constants.push(cc.name.to_string());
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

                let mut trait_def = ClassDef {
                    name: t.name.to_string(),
                    fqn: fqn(ns, t.name),
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
                };

                for member in t.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mdoc = docblock_before(source, member.span.start);
                            let mstart = view.position_of(member.span.start).line;
                            let vis = method_visibility(m.visibility);
                            trait_def.methods.push(MethodDef {
                                name: m.name.to_string(),
                                is_static: m.is_static,
                                is_abstract: m.is_abstract,
                                visibility: vis,
                                params: extract_params(&m.params),
                                return_type: m.return_type.as_ref().map(format_type_hint),
                                doc: mdoc,
                                start_line: mstart,
                            });
                        }
                        ClassMemberKind::Property(p) => {
                            let vis = method_visibility(p.visibility);
                            trait_def.properties.push(PropertyDef {
                                name: p.name.to_string(),
                                is_static: p.is_static,
                                type_hint: p.type_hint.as_ref().map(format_type_hint),
                                visibility: vis,
                            });
                        }
                        ClassMemberKind::ClassConst(cc) => {
                            trait_def.constants.push(cc.name.to_string());
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

                let mut enum_def = ClassDef {
                    name: e.name.to_string(),
                    fqn: fqn(ns, e.name),
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
                };

                for member in e.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Case(c) => {
                            enum_def.cases.push(c.name.to_string());
                        }
                        EnumMemberKind::Method(m) => {
                            let mdoc = docblock_before(source, member.span.start);
                            let mstart = view.position_of(member.span.start).line;
                            let vis = method_visibility(m.visibility);
                            enum_def.methods.push(MethodDef {
                                name: m.name.to_string(),
                                is_static: m.is_static,
                                is_abstract: m.is_abstract,
                                visibility: vis,
                                params: extract_params(&m.params),
                                return_type: m.return_type.as_ref().map(format_type_hint),
                                doc: mdoc,
                                start_line: mstart,
                            });
                        }
                        EnumMemberKind::ClassConst(cc) => {
                            enum_def.constants.push(cc.name.to_string());
                        }
                        _ => {}
                    }
                }
                index.classes.push(enum_def);
            }

            // ── Top-level const ──────────────────────────────────────────────
            StmtKind::Const(consts) => {
                for c in consts.iter() {
                    index.constants.push(c.name.to_string());
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
            name: p.name.to_string(),
            type_hint: p.type_hint.as_ref().map(format_type_hint),
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
        assert_eq!(cls.name, "Greeter");
        assert_eq!(cls.kind, ClassKind::Class);
        assert_eq!(cls.start_line, 1);
        assert_eq!(cls.methods.len(), 1);
        let method = &cls.methods[0];
        assert_eq!(method.name, "greet");
        assert_eq!(method.return_type.as_deref(), Some("string"));
        assert_eq!(method.params.len(), 1);
        assert_eq!(method.params[0].name, "name");
        assert_eq!(method.params[0].type_hint.as_deref(), Some("string"));
    }

    #[test]
    fn extracts_function() {
        let src = "<?php\nfunction add(int $a, int $b): int {}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.functions.len(), 1);
        let f = &idx.functions[0];
        assert_eq!(f.name, "add");
        assert_eq!(f.return_type.as_deref(), Some("int"));
        assert_eq!(f.params.len(), 2);
    }

    #[test]
    fn extracts_namespace() {
        let src = "<?php\nnamespace App\\Services;\nclass Mailer {}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.namespace.as_deref(), Some("App\\Services"));
        assert_eq!(idx.classes[0].fqn, "App\\Services\\Mailer");
    }

    #[test]
    fn extracts_braced_namespace() {
        let src = "<?php\nnamespace App\\Models {\n    class User {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.namespace.as_deref(), Some("App\\Models"));
        assert_eq!(idx.classes[0].fqn, "App\\Models\\User");
    }

    #[test]
    fn extracts_interface() {
        let src = "<?php\ninterface Countable {\n    public function count(): int;\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.classes.len(), 1);
        assert_eq!(idx.classes[0].kind, ClassKind::Interface);
        assert_eq!(idx.classes[0].methods[0].name, "count");
        assert!(idx.classes[0].methods[0].is_abstract);
    }

    #[test]
    fn extracts_trait() {
        let src = "<?php\ntrait Loggable {\n    public function log(): void {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.classes[0].kind, ClassKind::Trait);
        assert_eq!(idx.classes[0].methods[0].name, "log");
    }

    #[test]
    fn extracts_enum_cases() {
        let src = "<?php\nenum Status { case Active; case Inactive; }";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        assert_eq!(idx.classes[0].kind, ClassKind::Enum);
        assert!(idx.classes[0].cases.contains(&"Active".to_string()));
        assert!(idx.classes[0].cases.contains(&"Inactive".to_string()));
    }

    #[test]
    fn extracts_class_properties_and_constants() {
        let src = "<?php\nclass Config {\n    public string $host;\n    const VERSION = '1.0';\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        let cls = &idx.classes[0];
        assert_eq!(cls.properties.len(), 1);
        assert_eq!(cls.properties[0].name, "host");
        assert_eq!(cls.constants, vec!["VERSION"]);
    }

    #[test]
    fn extracts_trait_use() {
        let src = "<?php\ntrait T {}\nclass MyClass { use T; }";
        let doc = ParsedDoc::parse(src.to_string());
        let idx = FileIndex::extract(&doc);
        let cls = idx.classes.iter().find(|c| c.name == "MyClass").unwrap();
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
            cls.properties.iter().any(|p| p.name == "name"),
            "expected promoted property 'name', got: {:?}",
            cls.properties.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
    }
}
