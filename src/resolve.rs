//! Centralized cursor resolution.
//!
//! `goto_definition`, `goto_declaration`, and `hover` all needed to answer the
//! same question — *"which declaration in this AST is named `word`?"* — and each
//! had its own near-identical statement walker (`scan_statements`,
//! `find_any_declaration`, …). [`resolve_declaration`] is the single walker; it returns
//! a borrowed handle to the matched node ([`Declaration`]) and leaves rendering (range
//! vs. signature vs. abstract-filtering) to the caller.
//!
//! The walker performs *name matching*, not full cursor-context classification:
//! it matches a declaration whose name equals `word`, exactly as the three
//! original copies did. Distinguishing "method call vs. class name at this
//! offset" is a separate, behavior-changing concern and intentionally not done
//! here.
//!
//! Callers narrow the match with an `accept` predicate. Returning `false` means
//! "skip this candidate and keep looking", mirroring the `_ => {}` fall-through
//! in the original walkers. This is what lets declaration's two-pass logic
//! (abstract first, then any) reuse the same traversal.

use php_ast::{
    ClassConstDecl, ClassDecl, ClassMemberKind, EnumCase, EnumDecl, EnumMemberKind, FunctionDecl,
    Ident, InterfaceDecl, MethodDecl, NamespaceBody, Param, PropertyDecl, Span, Stmt, StmtKind,
    TraitDecl,
};

use crate::util::strip_variable_sigil;

/// Which type-like declaration a member belongs to. Lets callers reproduce
/// per-container behavior (e.g. definition resolves enum constants differently
/// from class constants) without re-walking the AST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Class,
    Interface,
    Trait,
    Enum,
}

/// A declaration node matched by name. Each variant borrows the matched AST node
/// (arena-allocated, so it outlives the walk) plus the span(s) callers need to
/// compute a precise name range.
pub enum Declaration<'a> {
    Function {
        decl: &'a FunctionDecl<'a, 'a>,
        stmt_span: Span,
    },
    Class {
        decl: &'a ClassDecl<'a, 'a>,
        /// The class name (always present — anonymous classes never match by name).
        name: Ident<'a>,
        stmt_span: Span,
    },
    Interface {
        decl: &'a InterfaceDecl<'a, 'a>,
        stmt_span: Span,
    },
    Trait {
        decl: &'a TraitDecl<'a, 'a>,
        stmt_span: Span,
    },
    Enum {
        decl: &'a EnumDecl<'a, 'a>,
        stmt_span: Span,
    },
    Method {
        method: &'a MethodDecl<'a, 'a>,
        container: Container,
        member_span: Span,
    },
    ClassConst {
        konst: &'a ClassConstDecl<'a, 'a>,
        container: Container,
        member_span: Span,
    },
    Property {
        property: &'a PropertyDecl<'a, 'a>,
        container: Container,
        member_span: Span,
    },
    /// A constructor-promoted parameter, which acts as a property declaration.
    PromotedParam { param: &'a Param<'a, 'a> },
    EnumCase {
        case: &'a EnumCase<'a, 'a>,
        enum_name: Ident<'a>,
        member_span: Span,
    },
}

impl<'a> Declaration<'a> {
    /// The identifier the cursor matched (without any `$` sigil).
    pub fn name(&self) -> &'a str {
        match self {
            Declaration::Function { decl, .. } => decl.name.or_error(),
            Declaration::Class { name, .. } => name.or_error(),
            Declaration::Interface { decl, .. } => decl.name.or_error(),
            Declaration::Trait { decl, .. } => decl.name.or_error(),
            Declaration::Enum { decl, .. } => decl.name.or_error(),
            Declaration::Method { method, .. } => method.name.or_error(),
            Declaration::ClassConst { konst, .. } => konst.name.or_error(),
            Declaration::Property { property, .. } => property.name.or_error(),
            Declaration::PromotedParam { param } => param.name.or_error(),
            Declaration::EnumCase { case, .. } => case.name.or_error(),
        }
    }
}

/// Find the first declaration named `word` that `accept` approves, scanning
/// `stmts` in source order and recursing into braced namespaces.
///
/// A `$` sigil on `word` is stripped before matching property / promoted-param
/// names (which are stored without it), matching the original walkers.
pub fn resolve_declaration<'a>(
    stmts: &'a [Stmt<'a, 'a>],
    word: &str,
    accept: &dyn Fn(&Declaration<'a>) -> bool,
) -> Option<Declaration<'a>> {
    let bare = strip_variable_sigil(word);
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                let d = Declaration::Function {
                    decl: f,
                    stmt_span: stmt.span,
                };
                if accept(&d) {
                    return Some(d);
                }
            }
            StmtKind::Class(c) => {
                // Class name takes priority over members (match-arm order in the
                // originals); fall through to members when the name is rejected.
                if let Some(name) = c.name
                    && name.or_error() == word
                {
                    let d = Declaration::Class {
                        decl: c,
                        name,
                        stmt_span: stmt.span,
                    };
                    if accept(&d) {
                        return Some(d);
                    }
                }
                if let Some(d) =
                    resolve_member(c.body.members.iter(), word, bare, Container::Class, accept)
                {
                    return Some(d);
                }
            }
            StmtKind::Interface(i) => {
                if i.name == word {
                    let d = Declaration::Interface {
                        decl: i,
                        stmt_span: stmt.span,
                    };
                    if accept(&d) {
                        return Some(d);
                    }
                }
                if let Some(d) = resolve_member(
                    i.body.members.iter(),
                    word,
                    bare,
                    Container::Interface,
                    accept,
                ) {
                    return Some(d);
                }
            }
            StmtKind::Trait(t) => {
                if t.name == word {
                    let d = Declaration::Trait {
                        decl: t,
                        stmt_span: stmt.span,
                    };
                    if accept(&d) {
                        return Some(d);
                    }
                }
                if let Some(d) =
                    resolve_member(t.body.members.iter(), word, bare, Container::Trait, accept)
                {
                    return Some(d);
                }
            }
            StmtKind::Enum(e) => {
                if e.name == word {
                    let d = Declaration::Enum {
                        decl: e,
                        stmt_span: stmt.span,
                    };
                    if accept(&d) {
                        return Some(d);
                    }
                }
                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Case(c) if c.name == word => {
                            let d = Declaration::EnumCase {
                                case: c,
                                enum_name: e.name,
                                member_span: member.span,
                            };
                            if accept(&d) {
                                return Some(d);
                            }
                        }
                        EnumMemberKind::Method(m) if m.name == word => {
                            let d = Declaration::Method {
                                method: m,
                                container: Container::Enum,
                                member_span: member.span,
                            };
                            if accept(&d) {
                                return Some(d);
                            }
                        }
                        EnumMemberKind::ClassConst(cc) if cc.name == word => {
                            let d = Declaration::ClassConst {
                                konst: cc,
                                container: Container::Enum,
                                member_span: member.span,
                            };
                            if accept(&d) {
                                return Some(d);
                            }
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(d) = resolve_declaration(&inner.stmts, word, accept)
                {
                    return Some(d);
                }
            }
            _ => {}
        }
    }
    None
}

/// Scan class/interface/trait body members. Promoted-constructor parameters are
/// only considered for `Container::Class` (where the originals handled them).
fn resolve_member<'a>(
    members: impl Iterator<Item = &'a php_ast::ClassMember<'a, 'a>>,
    word: &str,
    bare: &str,
    container: Container,
    accept: &dyn Fn(&Declaration<'a>) -> bool,
) -> Option<Declaration<'a>> {
    for member in members {
        match &member.kind {
            ClassMemberKind::Method(m) => {
                if m.name == word {
                    let d = Declaration::Method {
                        method: m,
                        container,
                        member_span: member.span,
                    };
                    if accept(&d) {
                        return Some(d);
                    }
                }
                // Constructor-promoted parameters act as property declarations.
                if container == Container::Class && m.name == "__construct" {
                    for p in m.params.iter() {
                        if p.visibility.is_some() && p.name == bare {
                            let d = Declaration::PromotedParam { param: p };
                            if accept(&d) {
                                return Some(d);
                            }
                        }
                    }
                }
            }
            ClassMemberKind::ClassConst(cc) if cc.name == word => {
                let d = Declaration::ClassConst {
                    konst: cc,
                    container,
                    member_span: member.span,
                };
                if accept(&d) {
                    return Some(d);
                }
            }
            ClassMemberKind::Property(p) if p.name == bare => {
                let d = Declaration::Property {
                    property: p,
                    container,
                    member_span: member.span,
                };
                if accept(&d) {
                    return Some(d);
                }
            }
            _ => {}
        }
    }
    None
}
