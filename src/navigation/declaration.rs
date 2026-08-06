/// `textDocument/declaration` — jump to the abstract or interface declaration of a symbol.
///
/// In PHP the distinction between declaration and definition matters for:
///   - Interface methods (declared but never given a body)
///   - Abstract class methods
///
/// For concrete symbols with no abstract counterpart this falls back to the same
/// result as go-to-definition so the request is never empty-handed.
use std::sync::Arc;

use tower_lsp_server::ls_types::{Location, Position, Uri};

use crate::document::ast::ParsedDoc;
use crate::lang::docblock::parse_docblock;
use crate::lang::is_unresolvable_bareword_at;
use crate::text::{strip_variable_sigil, word_at_position};
use crate::types::resolve::{Container, Declaration, resolve_declaration};

/// Find the abstract or interface declaration of `word`.
/// Prefers abstract/interface declarations; falls back to any declaration.
pub fn goto_declaration(
    source: &str,
    all_docs: &[(Uri, Arc<ParsedDoc>)],
    position: Position,
) -> Option<Location> {
    let word = word_at_position(source, position)?;

    // A bare keyword (`abstract`, `final`, `class`, ...) can never be a
    // declaration name. Without this, a same-named method elsewhere in an
    // open doc (semi-reserved words are valid method names) would match via
    // `resolve_declaration`'s bare-name search below. `goto_declaration_from_
    // index` already gates on this; this in-memory pass ran first and
    // unguarded, so it could return the wrong location before that check
    // ever ran.
    if is_unresolvable_bareword_at(source, position, &word) {
        return None;
    }

    // First pass: look for an abstract or interface declaration
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(decl) =
            resolve_declaration(&doc.program().stmts, &word, &is_abstract_declaration)
        {
            return Some(Location {
                uri: uri.clone(),
                range: sv.name_range_in_span(decl.name(), decl.span()),
            });
        }
    }

    // Second pass: any declaration (same as goto_definition)
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(decl) = resolve_declaration(&doc.program().stmts, &word, &is_any_declaration) {
            return Some(Location {
                uri: uri.clone(),
                range: sv.name_range_in_span(decl.name(), decl.span()),
            });
        }
    }

    None
}

/// Pass 1: abstract/interface declarations only — interface members and names,
/// plus abstract methods on classes and traits.
///
/// `resolve_declaration` checks a type's name before its members, whereas the original
/// walker checked interface members first. This only differs for an interface
/// named the same as a method it contains — syntactically legal but absurd, and
/// never seen in real PHP, so the order is harmless here.
fn is_abstract_declaration(decl: &Declaration<'_>) -> bool {
    match decl {
        Declaration::Interface { .. } => true,
        Declaration::Method {
            container: Container::Interface,
            ..
        } => true,
        Declaration::Method {
            method,
            container: Container::Class | Container::Trait,
            ..
        } => method.is_abstract,
        _ => false,
    }
}

/// Pass 2: any declaration. Constructor-promoted parameters are not surfaced as
/// declarations here (the original `find_any_declaration` never matched them).
fn is_any_declaration(decl: &Declaration<'_>) -> bool {
    !matches!(decl, Declaration::PromotedParam { .. })
}

/// Pass-2 body scoped to a single file: any declaration named `word`
/// (`bare` for properties, which are indexed without their `$` sigil).
/// Shared by the mention-index-narrowed fast path and the exhaustive
/// fallback scan below, so both paths apply identical matching rules.
pub(crate) fn any_declaration_in_doc(
    uri: &tower_lsp_server::ls_types::Uri,
    doc: &ParsedDoc,
    word: &str,
    bare: &str,
) -> Option<Location> {
    let sv = doc.view();
    any_declaration_in_stmts(uri, sv, &doc.program().stmts, word, bare)
}

/// Find abstract or interface declaration using the aggregated workspace
/// metadata plus salsa-backed parsed docs for unopened files.
pub fn goto_declaration_from_index(
    source: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    position: tower_lsp_server::ls_types::Position,
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
    mention_candidates: &dyn Fn(&str) -> Vec<tower_lsp_server::ls_types::Uri>,
) -> Option<Location> {
    use crate::text::word_at_position;
    let word = word_at_position(source, position)?;
    // A bare keyword (`abstract`, `final`, `class`, ...) can never be a
    // declaration name — bail out before either full-workspace scan below.
    // No candidate would ever declare a bare keyword anyway, so without this
    // check every keyword click pays for the exhaustive fallback loop.
    if is_unresolvable_bareword_at(source, position, &word) {
        return None;
    }
    let bare = strip_variable_sigil(&word);
    let candidate_uris = mention_candidates(&word);

    for uri in &candidate_uris {
        let Some(doc) = get_doc(uri) else { continue };
        if let Some(loc) = abstract_declaration_in_doc(uri, &doc, &word) {
            return Some(loc);
        }
    }

    // Second pass: any declaration. mir's mention index gives a candidate
    // file list for the common kinds (function/class/method/constant/
    // enum-case, all keyed by `word`), so the usual case scans one file's
    // classes instead of every class in every workspace file. Properties
    // keyed under `bare`, `@method` doc-methods (not indexed at all), and
    // any candidate miss fall back to the exhaustive scan below, identical
    // to the original behavior.
    for uri in &candidate_uris {
        if let Some(doc) = get_doc(uri)
            && let Some(loc) = any_declaration_in_doc(uri, &doc, &word, bare)
        {
            return Some(loc);
        }
    }

    for (uri, _) in &wi.files {
        let Some(doc) = get_doc(uri) else { continue };
        if let Some(loc) = any_declaration_in_doc(uri, &doc, &word, bare) {
            return Some(loc);
        }
    }
    None
}

fn abstract_declaration_in_doc(uri: &Uri, doc: &ParsedDoc, word: &str) -> Option<Location> {
    let sv = doc.view();
    resolve_declaration(&doc.program().stmts, word, &is_abstract_declaration).map(|decl| Location {
        uri: uri.clone(),
        range: sv.name_range_in_span(decl.name(), decl.span()),
    })
}

fn any_declaration_in_stmts(
    uri: &Uri,
    sv: crate::document::ast::SourceView<'_>,
    stmts: &[php_ast::Stmt<'_, '_>],
    word: &str,
    bare: &str,
) -> Option<Location> {
    for stmt in stmts {
        match &stmt.kind {
            php_ast::StmtKind::Function(f) if f.name == word => {
                return Some(Location {
                    uri: uri.clone(),
                    range: sv.name_range_in_span(f.name.or_error(), stmt.span),
                });
            }
            php_ast::StmtKind::Class(c) => {
                if let Some(name) = c.name
                    && name == word
                {
                    return Some(Location {
                        uri: uri.clone(),
                        range: sv.name_range_in_span(name.or_error(), stmt.span),
                    });
                }
                let scope = ClassLikeScope {
                    uri,
                    sv,
                    stmt_span: stmt.span,
                    owner_name: name_text(c.name),
                    doc_comment: c.doc_comment,
                };
                if let Some(loc) =
                    any_class_like_declaration_in_members(&scope, &c.body.members, word, bare)
                {
                    return Some(loc);
                }
            }
            php_ast::StmtKind::Interface(i) => {
                if i.name == word {
                    return Some(Location {
                        uri: uri.clone(),
                        range: sv.name_range_in_span(i.name.or_error(), stmt.span),
                    });
                }
                let scope = ClassLikeScope {
                    uri,
                    sv,
                    stmt_span: stmt.span,
                    owner_name: Some(i.name.or_error()),
                    doc_comment: i.doc_comment,
                };
                if let Some(loc) =
                    any_class_like_declaration_in_members(&scope, &i.body.members, word, bare)
                {
                    return Some(loc);
                }
            }
            php_ast::StmtKind::Trait(t) => {
                if t.name == word {
                    return Some(Location {
                        uri: uri.clone(),
                        range: sv.name_range_in_span(t.name.or_error(), stmt.span),
                    });
                }
                let scope = ClassLikeScope {
                    uri,
                    sv,
                    stmt_span: stmt.span,
                    owner_name: Some(t.name.or_error()),
                    doc_comment: t.doc_comment,
                };
                if let Some(loc) =
                    any_class_like_declaration_in_members(&scope, &t.body.members, word, bare)
                {
                    return Some(loc);
                }
            }
            php_ast::StmtKind::Enum(e) => {
                if e.name == word {
                    return Some(Location {
                        uri: uri.clone(),
                        range: sv.name_range_in_span(e.name.or_error(), stmt.span),
                    });
                }
                for member in e.body.members.iter() {
                    match &member.kind {
                        php_ast::EnumMemberKind::Method(m) if m.name == word => {
                            return Some(Location {
                                uri: uri.clone(),
                                range: sv.name_range_in_span(m.name.or_error(), member.span),
                            });
                        }
                        php_ast::EnumMemberKind::Case(c) if c.name == word => {
                            return Some(Location {
                                uri: uri.clone(),
                                range: sv.name_range_in_span(e.name.or_error(), stmt.span),
                            });
                        }
                        php_ast::EnumMemberKind::ClassConst(cc) if cc.name == word => {
                            return Some(Location {
                                uri: uri.clone(),
                                range: sv.name_range_in_span(e.name.or_error(), stmt.span),
                            });
                        }
                        _ => {}
                    }
                }
                if let Some(doc_comment) = e.doc_comment
                    && let Some(line) = doc_method_tag_line(sv, doc_comment, word)
                {
                    return Some(Location {
                        uri: uri.clone(),
                        range: crate::text::zero_width_range(line),
                    });
                }
            }
            php_ast::StmtKind::Namespace(ns) => {
                if let php_ast::NamespaceBody::Braced(inner) = &ns.body
                    && let Some(loc) = any_declaration_in_stmts(uri, sv, &inner.stmts, word, bare)
                {
                    return Some(loc);
                }
            }
            _ => {}
        }
    }
    None
}

fn any_class_like_declaration_in_members(
    scope: &ClassLikeScope<'_>,
    members: &[php_ast::ClassMember<'_, '_>],
    word: &str,
    bare: &str,
) -> Option<Location> {
    for member in members {
        match &member.kind {
            php_ast::ClassMemberKind::Method(m) if m.name == word => {
                return Some(Location {
                    uri: scope.uri.clone(),
                    range: scope.sv.name_range_in_span(m.name.or_error(), member.span),
                });
            }
            php_ast::ClassMemberKind::Property(p) if p.name == bare => {
                return Some(Location {
                    uri: scope.uri.clone(),
                    range: scope.sv.name_range_in_span(p.name.or_error(), member.span),
                });
            }
            php_ast::ClassMemberKind::ClassConst(cc) if cc.name == word => {
                return Some(Location {
                    uri: scope.uri.clone(),
                    range: scope
                        .owner_name
                        .map(|name| scope.sv.name_range_in_span(name, scope.stmt_span))
                        .unwrap_or_else(|| scope.sv.range_of(scope.stmt_span)),
                });
            }
            _ => {}
        }
    }

    if let Some(doc_comment) = scope.doc_comment
        && let Some(line) = doc_method_tag_line(scope.sv, doc_comment, word)
    {
        return Some(Location {
            uri: scope.uri.clone(),
            range: crate::text::zero_width_range(line),
        });
    }

    None
}

fn name_text(name: Option<php_ast::Ident<'_>>) -> Option<&str> {
    name.map(|n| n.or_error())
}

struct ClassLikeScope<'a> {
    uri: &'a Uri,
    sv: crate::document::ast::SourceView<'a>,
    stmt_span: php_ast::Span,
    owner_name: Option<&'a str>,
    doc_comment: Option<php_ast::Comment<'a>>,
}

fn doc_method_tag_line(
    view: crate::document::ast::SourceView<'_>,
    doc_comment: php_ast::Comment<'_>,
    method_name: &str,
) -> Option<u32> {
    let methods = parse_docblock(doc_comment.text).methods;
    if !methods
        .iter()
        .any(|m| m.name.eq_ignore_ascii_case(method_name))
    {
        return None;
    }
    let text = doc_comment.text;
    let base = doc_comment.span.start as usize;
    let mut offset = 0usize;
    while let Some(tag_pos) = text[offset..].find("@method") {
        let segment_start = offset + tag_pos;
        let segment = &text[segment_start..];
        let line_len = segment.find('\n').unwrap_or(segment.len());
        let needle = format!("{}(", method_name);
        if segment[..line_len]
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
        {
            return Some(view.position_of((base + segment_start) as u32).line);
        }
        offset = segment_start + "@method".len();
    }
    Some(view.position_of(doc_comment.span.start).line)
}
