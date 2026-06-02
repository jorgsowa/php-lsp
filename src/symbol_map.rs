//! Per-file memoized symbol table.
//!
//! [`SymbolMap`] is a pre-computed `HashMap<name, Vec<SymbolEntry>>` built from
//! a parsed PHP file in one AST pass. Each entry stores the precise LSP `Range`
//! of the identifier, the declaration kind, whether it is abstract, a
//! pre-rendered hover signature, and a pre-extracted docblock (as markdown).
//!
//! Because building the map is O(AST_size) but lookup is O(1), the payoff is
//! on the cross-file / `other_docs` path: a stable file (one that hasn't changed
//! since the last keystroke) has its map served from the salsa cache rather than
//! re-walking its AST on every request. See [`crate::db::symbol_map`] for the
//! salsa query that drives this.

use std::collections::HashMap;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::Range;

use crate::ast::ParsedDoc;
use crate::docblock::docblock_before;
use crate::hover::formatting::declaration_signature;
use crate::resolve::{Container, Declaration};

// ── Public types ──────────────────────────────────────────────────────────────

/// Which kind of PHP declaration this entry represents. Mirrors the variants of
/// [`Declaration`] so callers can reconstruct any accept predicate without an
/// AST walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolEntryKind {
    Function,
    Class,
    Interface,
    Trait,
    Enum,
    Method { container: Container },
    ClassConst { container: Container },
    Property { container: Container },
    PromotedParam,
    EnumCase,
}

/// A single resolved declaration stored in the pre-computed symbol map.
#[derive(Debug, Clone)]
pub struct SymbolEntry {
    /// Precise LSP range of the identifier (not the full declaration span).
    pub name_range: Range,
    pub kind: SymbolEntryKind,
    /// Whether the declaration is abstract (interface members, abstract methods).
    /// Used to reconstruct `goto_declaration`'s two-pass abstract-first logic.
    pub is_abstract: bool,
    /// Pre-rendered hover signature (e.g. `function foo(int $x): void`).
    /// `None` for properties and promoted parameters, which use the mir path.
    pub signature: Option<String>,
    /// Pre-extracted docblock rendered as markdown. `None` when no docblock
    /// precedes the declaration.
    pub doc_markdown: Option<String>,
}

/// Pre-computed symbol table for a single PHP file.
///
/// Built by [`SymbolMap::build`] in one AST pass; looked up in O(1) via
/// [`SymbolMap::lookup`]. The `Vec` per key preserves source order so that
/// predicates applied by [`lookup`] (e.g. "abstract first") stay correct.
#[derive(Clone, Default)]
pub struct SymbolMap {
    entries: HashMap<String, Vec<SymbolEntry>>,
}

impl SymbolMap {
    /// Walk `doc`'s AST once and build the complete symbol map.
    pub fn build(doc: &ParsedDoc) -> Self {
        let sv = doc.view();
        let source = sv.source();
        let mut entries: HashMap<String, Vec<SymbolEntry>> = HashMap::new();
        collect_stmts(&doc.program().stmts, source, sv, &mut entries);
        SymbolMap { entries }
    }

    /// Find the first entry with key `name` that `accept` approves, in source
    /// order — matching [`resolve_declaration`]'s first-match semantics.
    pub fn lookup(
        &self,
        name: &str,
        accept: impl Fn(&SymbolEntry) -> bool,
    ) -> Option<&SymbolEntry> {
        self.entries.get(name)?.iter().find(|e| accept(e))
    }

    /// Number of distinct symbol names (for size estimation / tests).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// ── AST walker ────────────────────────────────────────────────────────────────

fn collect_stmts<'a>(
    stmts: &'a [Stmt<'a, 'a>],
    source: &str,
    sv: crate::ast::SourceView<'_>,
    out: &mut HashMap<String, Vec<SymbolEntry>>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let Some(name) = f.name.as_str() else {
                    continue;
                };
                let decl = Declaration::Function {
                    decl: f,
                    stmt_span: stmt.span,
                };
                let sig = declaration_signature(&decl, name);
                let doc_markdown = docblock_before(source, stmt.span.start)
                    .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                    .filter(|md| !md.is_empty());
                push(
                    out,
                    name.to_owned(),
                    SymbolEntry {
                        name_range: sv.name_range_in_span(name, stmt.span),
                        kind: SymbolEntryKind::Function,
                        is_abstract: false,
                        signature: sig,
                        doc_markdown,
                    },
                );
            }

            StmtKind::Class(c) => {
                // Class name entry.
                if let Some(name_ident) = c.name {
                    let name = name_ident.or_error();
                    let decl = Declaration::Class {
                        decl: c,
                        name: name_ident,
                        stmt_span: stmt.span,
                    };
                    let sig = declaration_signature(&decl, name);
                    let doc_markdown = docblock_before(source, stmt.span.start)
                        .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                        .filter(|md| !md.is_empty());
                    push(
                        out,
                        name.to_owned(),
                        SymbolEntry {
                            name_range: sv.name_range_in_span(name, stmt.span),
                            kind: SymbolEntryKind::Class,
                            is_abstract: c.modifiers.is_abstract,
                            signature: sig,
                            doc_markdown,
                        },
                    );
                }
                collect_members(c.body.members.iter(), source, sv, Container::Class, out);
            }

            StmtKind::Interface(i) => {
                let name = i.name.or_error();
                let decl = Declaration::Interface {
                    decl: i,
                    stmt_span: stmt.span,
                };
                let sig = declaration_signature(&decl, name);
                let doc_markdown = docblock_before(source, stmt.span.start)
                    .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                    .filter(|md| !md.is_empty());
                push(
                    out,
                    name.to_owned(),
                    SymbolEntry {
                        name_range: sv.name_range_in_span(name, stmt.span),
                        kind: SymbolEntryKind::Interface,
                        is_abstract: true,
                        signature: sig,
                        doc_markdown,
                    },
                );
                collect_members(i.body.members.iter(), source, sv, Container::Interface, out);
            }

            StmtKind::Trait(t) => {
                let name = t.name.or_error();
                let decl = Declaration::Trait {
                    decl: t,
                    stmt_span: stmt.span,
                };
                let sig = declaration_signature(&decl, name);
                let doc_markdown = docblock_before(source, stmt.span.start)
                    .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                    .filter(|md| !md.is_empty());
                push(
                    out,
                    name.to_owned(),
                    SymbolEntry {
                        name_range: sv.name_range_in_span(name, stmt.span),
                        kind: SymbolEntryKind::Trait,
                        is_abstract: false,
                        signature: sig,
                        doc_markdown,
                    },
                );
                collect_members(t.body.members.iter(), source, sv, Container::Trait, out);
            }

            StmtKind::Enum(e) => {
                let name = e.name.or_error();
                let decl = Declaration::Enum {
                    decl: e,
                    stmt_span: stmt.span,
                };
                let sig = declaration_signature(&decl, name);
                let doc_markdown = docblock_before(source, stmt.span.start)
                    .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                    .filter(|md| !md.is_empty());
                push(
                    out,
                    name.to_owned(),
                    SymbolEntry {
                        name_range: sv.name_range_in_span(name, stmt.span),
                        kind: SymbolEntryKind::Enum,
                        is_abstract: false,
                        signature: sig,
                        doc_markdown,
                    },
                );

                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Case(c) => {
                            let case_name = c.name.or_error();
                            let case_decl = Declaration::EnumCase {
                                case: c,
                                enum_name: e.name,
                                member_span: member.span,
                            };
                            let sig = declaration_signature(&case_decl, case_name);
                            let doc_markdown = docblock_before(source, member.span.start)
                                .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                                .filter(|md| !md.is_empty());
                            push(
                                out,
                                case_name.to_owned(),
                                SymbolEntry {
                                    name_range: sv.name_range(case_name),
                                    kind: SymbolEntryKind::EnumCase,
                                    is_abstract: false,
                                    signature: sig,
                                    doc_markdown,
                                },
                            );
                        }
                        EnumMemberKind::Method(m) => {
                            let mname = m.name.or_error();
                            let m_decl = Declaration::Method {
                                method: m,
                                container: Container::Enum,
                                member_span: member.span,
                            };
                            let sig = declaration_signature(&m_decl, mname);
                            let doc_markdown = docblock_before(source, member.span.start)
                                .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                                .filter(|md| !md.is_empty());
                            push(
                                out,
                                mname.to_owned(),
                                SymbolEntry {
                                    name_range: sv.name_range(mname),
                                    kind: SymbolEntryKind::Method {
                                        container: Container::Enum,
                                    },
                                    is_abstract: false,
                                    signature: sig,
                                    doc_markdown,
                                },
                            );
                        }
                        EnumMemberKind::ClassConst(cc) => {
                            let cc_name = cc.name.or_error();
                            let cc_decl = Declaration::ClassConst {
                                konst: cc,
                                container: Container::Enum,
                                member_span: member.span,
                            };
                            let sig = declaration_signature(&cc_decl, cc_name);
                            let doc_markdown = docblock_before(source, member.span.start)
                                .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                                .filter(|md| !md.is_empty());
                            push(
                                out,
                                cc_name.to_owned(),
                                SymbolEntry {
                                    name_range: sv.name_range(cc_name),
                                    kind: SymbolEntryKind::ClassConst {
                                        container: Container::Enum,
                                    },
                                    is_abstract: false,
                                    signature: sig,
                                    doc_markdown,
                                },
                            );
                        }
                        _ => {}
                    }
                }
            }

            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_stmts(&inner.stmts, source, sv, out);
                }
            }

            _ => {}
        }
    }
}

fn collect_members<'a>(
    members: impl Iterator<Item = &'a php_ast::ClassMember<'a, 'a>>,
    source: &str,
    sv: crate::ast::SourceView<'_>,
    container: Container,
    out: &mut HashMap<String, Vec<SymbolEntry>>,
) {
    for member in members {
        match &member.kind {
            ClassMemberKind::Method(m) => {
                let mname = m.name.or_error();
                let m_decl = Declaration::Method {
                    method: m,
                    container,
                    member_span: member.span,
                };
                let sig = declaration_signature(&m_decl, mname);
                let doc_markdown = docblock_before(source, member.span.start)
                    .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                    .filter(|md| !md.is_empty());
                let name_range = if container == Container::Class {
                    sv.name_range_in_span(mname, member.span)
                } else {
                    sv.name_range(mname)
                };
                let is_abstract = match container {
                    Container::Interface => true,
                    Container::Class | Container::Trait => m.is_abstract,
                    Container::Enum => false,
                };
                push(
                    out,
                    mname.to_owned(),
                    SymbolEntry {
                        name_range,
                        kind: SymbolEntryKind::Method { container },
                        is_abstract,
                        signature: sig,
                        doc_markdown,
                    },
                );

                // Constructor-promoted parameters (only for Container::Class).
                if container == Container::Class && m.name == "__construct" {
                    for p in m.params.iter() {
                        if p.visibility.is_some() {
                            let pname = p.name.or_error();
                            let bare = pname.trim_start_matches('$');
                            push(
                                out,
                                bare.to_owned(),
                                SymbolEntry {
                                    name_range: sv.name_range_in_span(pname, p.span),
                                    kind: SymbolEntryKind::PromotedParam,
                                    is_abstract: false,
                                    signature: None,
                                    doc_markdown: None,
                                },
                            );
                        }
                    }
                }
            }

            ClassMemberKind::ClassConst(cc) => {
                let cc_name = cc.name.or_error();
                let cc_decl = Declaration::ClassConst {
                    konst: cc,
                    container,
                    member_span: member.span,
                };
                let sig = declaration_signature(&cc_decl, cc_name);
                let doc_markdown = docblock_before(source, member.span.start)
                    .map(|raw| crate::docblock::parse_docblock(&raw).to_markdown())
                    .filter(|md| !md.is_empty());
                let name_range = if container == Container::Class {
                    sv.name_range_in_span(cc_name, member.span)
                } else {
                    sv.name_range(cc_name)
                };
                push(
                    out,
                    cc_name.to_owned(),
                    SymbolEntry {
                        name_range,
                        kind: SymbolEntryKind::ClassConst { container },
                        is_abstract: false,
                        signature: sig,
                        doc_markdown,
                    },
                );
            }

            ClassMemberKind::Property(p) => {
                let pname = p.name.or_error();
                let bare = pname.trim_start_matches('$');
                // Properties: signature rendered via mir, not here.
                let name_range = if container == Container::Class {
                    sv.name_range_in_span(pname, member.span)
                } else {
                    sv.name_range(pname)
                };
                push(
                    out,
                    bare.to_owned(),
                    SymbolEntry {
                        name_range,
                        kind: SymbolEntryKind::Property { container },
                        is_abstract: false,
                        signature: None,
                        doc_markdown: None,
                    },
                );
            }

            _ => {}
        }
    }
}

fn push(out: &mut HashMap<String, Vec<SymbolEntry>>, key: String, entry: SymbolEntry) {
    out.entry(key).or_default().push(entry);
}

// ── Predicate helpers (mirrors resolve.rs / declaration.rs predicates) ────────

/// Reconstruct the `is_hoverable` predicate from a stored [`SymbolEntryKind`].
pub fn is_hoverable_kind(kind: SymbolEntryKind) -> bool {
    !matches!(
        kind,
        SymbolEntryKind::Property { .. } | SymbolEntryKind::PromotedParam
    )
}

/// `goto_declaration` pass 1: abstract/interface declarations.
pub fn is_abstract_entry(e: &SymbolEntry) -> bool {
    match e.kind {
        SymbolEntryKind::Interface => true,
        SymbolEntryKind::Method {
            container: Container::Interface,
        } => true,
        SymbolEntryKind::Method {
            container: Container::Class | Container::Trait,
        } => e.is_abstract,
        _ => false,
    }
}

/// `goto_declaration` pass 2: any declaration except promoted params.
pub fn is_any_entry(e: &SymbolEntry) -> bool {
    !matches!(e.kind, SymbolEntryKind::PromotedParam)
}

/// `goto_definition`: skip enum constants (matching original walker).
pub fn is_definition_entry(e: &SymbolEntry) -> bool {
    !matches!(
        e.kind,
        SymbolEntryKind::ClassConst {
            container: Container::Enum
        }
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn build(src: &str) -> SymbolMap {
        let doc = ParsedDoc::parse(src.to_owned());
        SymbolMap::build(&doc)
    }

    #[test]
    fn top_level_function() {
        let m = build("<?php\nfunction greet(string $name): string { return $name; }");
        let e = m.lookup("greet", |_| true).unwrap();
        assert_eq!(e.kind, SymbolEntryKind::Function);
        assert!(!e.is_abstract);
        assert_eq!(
            e.signature.as_deref(),
            Some("function greet(string $name): string")
        );
    }

    #[test]
    fn class_with_abstract_method() {
        let m = build("<?php\nabstract class Foo {\n    abstract public function bar(): void;\n}");
        let cls = m.lookup("Foo", |_| true).unwrap();
        assert_eq!(cls.kind, SymbolEntryKind::Class);
        assert!(cls.is_abstract);

        let method = m
            .lookup("bar", |e| {
                matches!(
                    e.kind,
                    SymbolEntryKind::Method {
                        container: Container::Class
                    }
                )
            })
            .unwrap();
        assert!(method.is_abstract);
    }

    #[test]
    fn interface_member_is_abstract() {
        let m = build("<?php\ninterface Shape {\n    public function area(): float;\n}");
        let method = m.lookup("area", |_| true).unwrap();
        assert!(method.is_abstract);
        assert_eq!(
            method.kind,
            SymbolEntryKind::Method {
                container: Container::Interface
            }
        );
    }

    #[test]
    fn enum_entries() {
        let m = build("<?php\nenum Color {\n    case Red;\n    case Blue;\n}");
        assert!(m.lookup("Color", |_| true).is_some());
        assert!(m.lookup("Red", |_| true).is_some());
        assert!(m.lookup("Blue", |_| true).is_some());
    }

    #[test]
    fn promoted_param_keyed_without_dollar() {
        let m = build(
            "<?php\nclass Point {\n    public function __construct(\n        public float $x,\n        public float $y,\n    ) {}\n}",
        );
        assert!(
            m.lookup("x", |e| matches!(e.kind, SymbolEntryKind::PromotedParam))
                .is_some()
        );
        assert!(
            m.lookup("y", |e| matches!(e.kind, SymbolEntryKind::PromotedParam))
                .is_some()
        );
    }

    #[test]
    fn source_order_preserved() {
        // Both `render` in Interface and Trait: interface entry must come before
        // trait entry so the abstract-first lookup finds the right one.
        let m = build(
            "<?php\ninterface I {\n    public function render(): void;\n}\ntrait T {\n    abstract public function render(): void;\n}",
        );
        let entries = m.entries.get("render").unwrap();
        assert_eq!(
            entries[0].kind,
            SymbolEntryKind::Method {
                container: Container::Interface
            }
        );
        assert_eq!(
            entries[1].kind,
            SymbolEntryKind::Method {
                container: Container::Trait
            }
        );
    }

    #[test]
    fn docblock_extracted() {
        let m = build("<?php\n/** Greets the user. */\nfunction greet(): void {}");
        let e = m.lookup("greet", |_| true).unwrap();
        assert!(
            e.doc_markdown.is_some(),
            "expected docblock to be extracted"
        );
    }

    #[test]
    fn no_docblock_when_absent() {
        let m = build("<?php\nfunction greet(): void {}");
        let e = m.lookup("greet", |_| true).unwrap();
        assert!(e.doc_markdown.is_none());
    }
}
