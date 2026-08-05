#![allow(deprecated)]

use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::{
    DocumentSymbol, Location, OneOf, Range, SymbolInformation, SymbolKind, Uri, WorkspaceSymbol,
};

use crate::document::ast::{ParsedDoc, SourceView, name_range};
use crate::lang::docblock::parse_docblock;
use crate::text::zero_width_range;

fn is_deprecated_doc(doc: Option<&php_ast::Comment<'_>>) -> Option<bool> {
    doc.and_then(|c| {
        if parse_docblock(c.text).deprecated.is_some() {
            Some(true)
        } else {
            None
        }
    })
}

pub fn document_symbols(_source: &str, doc: &ParsedDoc) -> Vec<DocumentSymbol> {
    let sv = doc.view();
    symbols_from_statements(sv, &doc.program().stmts)
}

/// A `Location`'s range is a workspace-symbol placeholder — `zero_width_range`
/// collapsed to the start of a line — rather than a genuinely precise, already-resolved range.
fn is_placeholder_range(range: &Range) -> bool {
    range.start == range.end && range.start.character == 0
}

/// Fill in the sv.source() range for a `WorkspaceSymbol` whose `location` carries only a URI
/// (i.e. `OneOf::Right(WorkspaceLocation)`) or a line-level placeholder range produced by
/// workspace symbol search. If the range is already precise, or if the document cannot
/// be found, the symbol is returned unchanged. If the symbol name is not found in the source,
/// the existing location is preserved.
pub fn resolve_workspace_symbol(
    mut symbol: WorkspaceSymbol,
    docs: &[(Uri, Arc<ParsedDoc>)],
) -> WorkspaceSymbol {
    let uri = match &symbol.location {
        // Genuinely precise range — already resolved, nothing to do.
        OneOf::Left(loc) if !is_placeholder_range(&loc.range) => return symbol,
        OneOf::Left(loc) => loc.uri.clone(),
        OneOf::Right(wl) => wl.uri.clone(),
    };
    for (doc_uri, doc) in docs {
        if doc_uri == &uri {
            if let Some(range) = name_range(doc.source(), doc.line_starts(), &symbol.name) {
                symbol.location = OneOf::Left(Location { uri, range });
            }
            break;
        }
    }
    symbol
}

/// Parse an optional kind-filter prefix from the query string.
///
/// Supported prefixes:
/// - `#class:` → `SymbolKind::CLASS`
/// - `#fn:` or `#function:` → `SymbolKind::FUNCTION`
/// - `#method:` → `SymbolKind::METHOD`
/// - `#interface:` → `SymbolKind::INTERFACE`
/// - `#enum:` → `SymbolKind::ENUM`
/// - `#const:` → `SymbolKind::CONSTANT`
/// - `#prop:` or `#property:` → `SymbolKind::PROPERTY`
///
/// Returns `(kind_filter, actual_search_term)`.
fn parse_kind_filter(query: &str) -> (Option<SymbolKind>, &str) {
    let Some(rest) = query.strip_prefix('#') else {
        return (None, query);
    };
    let (prefix, term) = match rest.split_once(':') {
        Some((p, t)) => (p, t),
        None => return (None, query),
    };
    let kind = match prefix.to_lowercase().as_str() {
        "class" | "c" => SymbolKind::CLASS,
        "fn" | "function" | "f" => SymbolKind::FUNCTION,
        "method" | "m" => SymbolKind::METHOD,
        "interface" | "i" => SymbolKind::INTERFACE,
        "enum" | "e" => SymbolKind::ENUM,
        "const" | "constant" => SymbolKind::CONSTANT,
        "prop" | "property" | "p" => SymbolKind::PROPERTY,
        _ => return (None, query),
    };
    (Some(kind), term)
}

fn symbols_from_statements(sv: SourceView<'_>, stmts: &[Stmt<'_, '_>]) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    symbols.extend(symbols_from_statements(sv, &inner.stmts));
                }
            }
            _ => {
                if let Some(sym) = statement_to_symbol(sv, stmt) {
                    symbols.push(sym);
                }
            }
        }
    }
    symbols
}

fn stmt_range(sv: SourceView<'_>, stmt: &Stmt<'_, '_>) -> Range {
    let start = sv.position_of(stmt.span.start);
    let end = sv.position_of(stmt.span.end);
    Range { start, end }
}

fn member_range(sv: SourceView<'_>, member: &php_ast::ClassMember<'_, '_>) -> Range {
    let start = sv.position_of(member.span.start);
    let end = sv.position_of(member.span.end);
    Range { start, end }
}

fn param_range(sv: SourceView<'_>, param: &php_ast::Param<'_, '_>) -> Range {
    let start = sv.position_of(param.span.start);
    let end = sv.position_of(param.span.end);
    Range { start, end }
}

/// Per-parameter document symbols, used both for top-level functions and for
/// class/trait methods — most importantly `__construct`, whose promoted
/// properties (`public readonly Foo $foo`) exist only as `Param` nodes and
/// would otherwise be entirely invisible in the outline.
fn param_children_for(
    sv: SourceView<'_>,
    params: &[php_ast::Param<'_, '_>],
) -> Option<Vec<DocumentSymbol>> {
    let children: Vec<DocumentSymbol> = params
        .iter()
        .map(|p| {
            let prange = param_range(sv, p);
            let psel = sv.name_range_after_attrs(&p.name.to_string(), &p.attributes, p.span);
            DocumentSymbol {
                name: format!("${}", p.name),
                detail: None,
                kind: SymbolKind::VARIABLE,
                tags: None,
                deprecated: None,
                range: prange,
                selection_range: psel,
                children: None,
            }
        })
        .collect();
    if children.is_empty() {
        None
    } else {
        Some(children)
    }
}

fn statement_to_symbol(sv: SourceView<'_>, stmt: &Stmt<'_, '_>) -> Option<DocumentSymbol> {
    match &stmt.kind {
        StmtKind::Function(f) => {
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&f.name.to_string(), &f.attributes, stmt.span);
            let detail = Some(format_fn_signature(&f.params, f.return_type.as_ref()));
            let is_deprecated = is_deprecated_doc(f.doc_comment.as_ref());

            Some(DocumentSymbol {
                name: f.name.to_string(),
                detail,
                kind: SymbolKind::FUNCTION,
                tags: None,
                deprecated: is_deprecated,
                range,
                selection_range,
                children: param_children_for(sv, &f.params),
            })
        }

        StmtKind::Class(c) => {
            let name = c.name?;
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&name.to_string(), &c.attributes, stmt.span);
            let class_deprecated = is_deprecated_doc(c.doc_comment.as_ref());

            let children: Vec<DocumentSymbol> = c
                .body
                .members
                .iter()
                .flat_map(|member| -> Vec<DocumentSymbol> {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mrange = member_range(sv, member);
                            let msel = sv.name_range_after_attrs(
                                &m.name.to_string(),
                                &m.attributes,
                                member.span,
                            );
                            let detail =
                                Some(format_fn_signature(&m.params, m.return_type.as_ref()));
                            let method_deprecated = is_deprecated_doc(m.doc_comment.as_ref());
                            vec![DocumentSymbol {
                                name: m.name.to_string(),
                                detail,
                                kind: SymbolKind::METHOD,
                                tags: None,
                                deprecated: method_deprecated,
                                range: mrange,
                                selection_range: msel,
                                children: param_children_for(sv, &m.params),
                            }]
                        }
                        ClassMemberKind::Property(p) => {
                            let prange = member_range(sv, member);
                            let psel = sv.name_range_after_attrs(
                                &p.name.to_string(),
                                &p.attributes,
                                member.span,
                            );
                            let prop_deprecated = is_deprecated_doc(p.doc_comment.as_ref());
                            vec![DocumentSymbol {
                                name: format!("${}", p.name),
                                detail: None,
                                kind: SymbolKind::PROPERTY,
                                tags: None,
                                deprecated: prop_deprecated,
                                range: prange,
                                selection_range: psel,
                                children: None,
                            }]
                        }
                        ClassMemberKind::ClassConst(cc) => {
                            let crange = member_range(sv, member);
                            let csel = sv.name_range_after_attrs(
                                &cc.name.to_string(),
                                &cc.attributes,
                                member.span,
                            );
                            let const_deprecated = is_deprecated_doc(cc.doc_comment.as_ref());
                            vec![DocumentSymbol {
                                name: cc.name.to_string(),
                                detail: None,
                                kind: SymbolKind::CONSTANT,
                                tags: None,
                                deprecated: const_deprecated,
                                range: crange,
                                selection_range: csel,
                                children: None,
                            }]
                        }
                        _ => vec![],
                    }
                })
                .collect();

            Some(DocumentSymbol {
                name: name.to_string(),
                detail: None,
                kind: SymbolKind::CLASS,
                tags: None,
                deprecated: class_deprecated,
                range,
                selection_range,
                children: if children.is_empty() {
                    None
                } else {
                    Some(children)
                },
            })
        }

        StmtKind::Interface(i) => {
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&i.name.to_string(), &i.attributes, stmt.span);
            let iface_deprecated = is_deprecated_doc(i.doc_comment.as_ref());
            let children: Vec<DocumentSymbol> = i
                .body
                .members
                .iter()
                .filter_map(|member| match &member.kind {
                    ClassMemberKind::Method(m) => {
                        let mrange = member_range(sv, member);
                        let msel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        let method_deprecated = is_deprecated_doc(m.doc_comment.as_ref());
                        Some(DocumentSymbol {
                            name: m.name.to_string(),
                            detail: Some(format_fn_signature(&m.params, m.return_type.as_ref())),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: method_deprecated,
                            range: mrange,
                            selection_range: msel,
                            children: None,
                        })
                    }
                    ClassMemberKind::ClassConst(cc) => {
                        let crange = member_range(sv, member);
                        let csel = sv.name_range_after_attrs(
                            &cc.name.to_string(),
                            &cc.attributes,
                            member.span,
                        );
                        let const_deprecated = is_deprecated_doc(cc.doc_comment.as_ref());
                        Some(DocumentSymbol {
                            name: cc.name.to_string(),
                            detail: None,
                            kind: SymbolKind::CONSTANT,
                            tags: None,
                            deprecated: const_deprecated,
                            range: crange,
                            selection_range: csel,
                            children: None,
                        })
                    }
                    _ => None,
                })
                .collect();
            Some(DocumentSymbol {
                name: i.name.to_string(),
                detail: None,
                kind: SymbolKind::INTERFACE,
                tags: None,
                deprecated: iface_deprecated,
                range,
                selection_range,
                children: if children.is_empty() {
                    None
                } else {
                    Some(children)
                },
            })
        }

        StmtKind::Trait(t) => {
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&t.name.to_string(), &t.attributes, stmt.span);
            let trait_deprecated = is_deprecated_doc(t.doc_comment.as_ref());
            let children: Vec<DocumentSymbol> = t
                .body
                .members
                .iter()
                .filter_map(|member| {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let mrange = member_range(sv, member);
                        let msel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        let method_deprecated = is_deprecated_doc(m.doc_comment.as_ref());
                        Some(DocumentSymbol {
                            name: m.name.to_string(),
                            detail: Some(format_fn_signature(&m.params, m.return_type.as_ref())),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: method_deprecated,
                            range: mrange,
                            selection_range: msel,
                            children: param_children_for(sv, &m.params),
                        })
                    } else {
                        None
                    }
                })
                .collect();

            Some(DocumentSymbol {
                name: t.name.to_string(),
                detail: None,
                kind: SymbolKind::CLASS,
                tags: None,
                deprecated: trait_deprecated,
                range,
                selection_range,
                children: if children.is_empty() {
                    None
                } else {
                    Some(children)
                },
            })
        }

        StmtKind::Enum(e) => {
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&e.name.to_string(), &e.attributes, stmt.span);
            let enum_deprecated = is_deprecated_doc(e.doc_comment.as_ref());
            let children: Vec<DocumentSymbol> = e
                .body
                .members
                .iter()
                .filter_map(|member| match &member.kind {
                    EnumMemberKind::Case(c) => {
                        let crange = Range {
                            start: sv.position_of(member.span.start),
                            end: sv.position_of(member.span.end),
                        };
                        let csel = sv.name_range_after_attrs(
                            &c.name.to_string(),
                            &c.attributes,
                            member.span,
                        );
                        Some(DocumentSymbol {
                            name: c.name.to_string(),
                            detail: None,
                            kind: SymbolKind::ENUM_MEMBER,
                            tags: None,
                            deprecated: None,
                            range: crange,
                            selection_range: csel,
                            children: None,
                        })
                    }
                    EnumMemberKind::Method(m) => {
                        let mrange = Range {
                            start: sv.position_of(member.span.start),
                            end: sv.position_of(member.span.end),
                        };
                        let msel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        let method_deprecated = is_deprecated_doc(m.doc_comment.as_ref());
                        Some(DocumentSymbol {
                            name: m.name.to_string(),
                            detail: Some(format_fn_signature(&m.params, m.return_type.as_ref())),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: method_deprecated,
                            range: mrange,
                            selection_range: msel,
                            children: None,
                        })
                    }
                    EnumMemberKind::ClassConst(cc) => {
                        let crange = Range {
                            start: sv.position_of(member.span.start),
                            end: sv.position_of(member.span.end),
                        };
                        let csel = sv.name_range_after_attrs(
                            &cc.name.to_string(),
                            &cc.attributes,
                            member.span,
                        );
                        let const_deprecated = is_deprecated_doc(cc.doc_comment.as_ref());
                        Some(DocumentSymbol {
                            name: cc.name.to_string(),
                            detail: None,
                            kind: SymbolKind::CONSTANT,
                            tags: None,
                            deprecated: const_deprecated,
                            range: crange,
                            selection_range: csel,
                            children: None,
                        })
                    }
                    _ => None,
                })
                .collect();

            Some(DocumentSymbol {
                name: e.name.to_string(),
                detail: None,
                kind: SymbolKind::ENUM,
                tags: None,
                deprecated: enum_deprecated,
                range,
                selection_range,
                children: if children.is_empty() {
                    None
                } else {
                    Some(children)
                },
            })
        }

        _ => None,
    }
}

fn format_fn_signature(
    params: &[php_ast::Param<'_, '_>],
    ret: Option<&php_ast::TypeHint<'_, '_>>,
) -> String {
    use crate::document::ast::format_type_hint;
    let params_str = params
        .iter()
        .map(|p| {
            let mut s = String::new();
            if p.by_ref {
                s.push('&');
            }
            if p.variadic {
                s.push_str("...");
            }
            if let Some(t) = &p.type_hint {
                s.push_str(&format!("{} ", format_type_hint(t)));
            }
            s.push_str(&format!("${}", p.name));
            s
        })
        .collect::<Vec<_>>()
        .join(", ");
    let ret_str = ret
        .map(|r| format!(": {}", format_type_hint(r)))
        .unwrap_or_default();
    format!("({}){}", params_str, ret_str)
}

pub fn workspace_symbols_from_workspace(
    query: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
    get_doc: &dyn Fn(&Uri) -> Option<Arc<ParsedDoc>>,
) -> Vec<SymbolInformation> {
    let (kind_filter, term) = parse_kind_filter(query);
    let matches_kind = |k: SymbolKind| kind_filter.is_none_or(|f| f == k);
    let fq = crate::text::FuzzyQuery::new(term);

    let mut results = Vec::new();
    for (uri, _) in &wi.files {
        let Some(doc) = get_doc(uri) else { continue };
        for sym in document_symbols(doc.source(), &doc) {
            collect_workspace_symbols(
                uri,
                &sym,
                None,
                sym.selection_range.start.line,
                &matches_kind,
                &fq,
                &mut results,
            );
        }
    }
    results
}

fn collect_workspace_symbols(
    uri: &Uri,
    sym: &DocumentSymbol,
    container_name: Option<&str>,
    container_line: u32,
    matches_kind: &dyn Fn(SymbolKind) -> bool,
    fq: &crate::text::FuzzyQuery,
    out: &mut Vec<SymbolInformation>,
) {
    if sym.kind == SymbolKind::VARIABLE {
        return;
    }
    let line = match sym.kind {
        SymbolKind::CONSTANT | SymbolKind::ENUM_MEMBER => container_line,
        _ => sym.selection_range.start.line,
    };
    if matches_kind(sym.kind) && fq.symbol_match(&sym.name) {
        out.push(SymbolInformation {
            name: sym.name.clone(),
            kind: sym.kind,
            location: Location {
                uri: uri.clone(),
                range: zero_width_range(line),
            },
            tags: sym.tags.clone(),
            deprecated: sym.deprecated,
            container_name: container_name.map(str::to_string),
        });
    }
    if let Some(children) = &sym.children {
        for child in children {
            collect_workspace_symbols(
                uri,
                child,
                Some(&sym.name),
                sym.selection_range.start.line,
                matches_kind,
                fq,
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind_filter_extracts_class_prefix() {
        let (kind, term) = parse_kind_filter("#class:MyClass");
        assert_eq!(kind, Some(SymbolKind::CLASS));
        assert_eq!(term, "MyClass");
    }

    #[test]
    fn parse_kind_filter_no_prefix_returns_none() {
        let (kind, term) = parse_kind_filter("MyClass");
        assert_eq!(kind, None);
        assert_eq!(term, "MyClass");
    }
}
