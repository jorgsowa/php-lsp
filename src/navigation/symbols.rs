#![allow(deprecated)]

use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::{
    DocumentSymbol, Location, OneOf, Range, SymbolInformation, SymbolKind, Uri, WorkspaceSymbol,
};

use crate::document::ast::{ParsedDoc, SourceView, name_range};
use crate::index::file_index::WorkspaceSymbolRow;
use crate::lang::docblock::parse_docblock;
use crate::text::zero_width_range;

fn is_deprecated_doc(doc: Option<&php_ast::Comment<'_>>) -> Option<bool> {
    doc.and_then(|c| {
        // Cheap pre-check: the docblock parser only matches the literal `@deprecated` tag.
        if c.text.contains("@deprecated") && parse_docblock(c.text).deprecated.is_some() {
            Some(true)
        } else {
            None
        }
    })
}

pub fn document_symbols(_source: &str, doc: &ParsedDoc) -> Vec<DocumentSymbol> {
    let sv = doc.view();
    symbols_from_statements(sv, &doc.program().stmts, true)
}

/// `document_symbols` without signature details or parameter children.
fn outline_symbols(doc: &ParsedDoc) -> Vec<DocumentSymbol> {
    symbols_from_statements(doc.view(), &doc.program().stmts, false)
}

fn fn_detail(
    full: bool,
    params: &[php_ast::Param<'_, '_>],
    ret: Option<&php_ast::TypeHint<'_, '_>>,
) -> Option<String> {
    full.then(|| format_fn_signature(params, ret))
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

fn symbols_from_statements(
    sv: SourceView<'_>,
    stmts: &[Stmt<'_, '_>],
    full: bool,
) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    symbols.extend(symbols_from_statements(sv, &inner.stmts, full));
                }
            }
            _ => {
                if let Some(sym) = statement_to_symbol(sv, stmt, full) {
                    symbols.push(sym);
                }
            }
        }
    }
    symbols
}

/// Error recovery can leave a span ending before it starts; clamp it to empty.
fn span_range(sv: SourceView<'_>, span: php_ast::Span) -> Range {
    let start = sv.position_of(span.start);
    let end = sv.position_of(span.end.max(span.start));
    Range { start, end }
}

fn stmt_range(sv: SourceView<'_>, stmt: &Stmt<'_, '_>) -> Range {
    span_range(sv, stmt.span)
}

fn member_range(sv: SourceView<'_>, member: &php_ast::ClassMember<'_, '_>) -> Range {
    span_range(sv, member.span)
}

fn param_range(sv: SourceView<'_>, param: &php_ast::Param<'_, '_>) -> Range {
    span_range(sv, param.span)
}

/// Per-parameter document symbols, used both for top-level functions and for
/// class/trait methods. Promoted constructor properties are emitted separately
/// as class-level `Property` symbols, so this helper intentionally keeps
/// parameters as parameters-only.
fn param_children_for(
    full: bool,
    sv: SourceView<'_>,
    params: &[php_ast::Param<'_, '_>],
) -> Option<Vec<DocumentSymbol>> {
    if !full {
        return None;
    }
    let children: Vec<DocumentSymbol> = params
        .iter()
        .filter(|p| p.visibility.is_none() && !p.name.is_error())
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

fn promoted_property_symbols_for(
    sv: SourceView<'_>,
    params: &[php_ast::Param<'_, '_>],
) -> Vec<DocumentSymbol> {
    params
        .iter()
        .filter(|p| p.visibility.is_some() && !p.name.is_error())
        .map(|p| {
            let prange = param_range(sv, p);
            let psel = sv.name_range_after_attrs(&p.name.to_string(), &p.attributes, p.span);
            DocumentSymbol {
                name: format!("${}", p.name),
                detail: None,
                kind: SymbolKind::PROPERTY,
                tags: None,
                deprecated: None,
                range: prange,
                selection_range: psel,
                children: None,
            }
        })
        .collect()
}

fn statement_to_symbol(
    sv: SourceView<'_>,
    stmt: &Stmt<'_, '_>,
    full: bool,
) -> Option<DocumentSymbol> {
    match &stmt.kind {
        StmtKind::Function(f) if !f.name.is_error() => {
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&f.name.to_string(), &f.attributes, stmt.span);
            let detail = fn_detail(full, &f.params, f.return_type.as_ref());
            let is_deprecated = is_deprecated_doc(f.doc_comment.as_ref());

            Some(DocumentSymbol {
                name: f.name.to_string(),
                detail,
                kind: SymbolKind::FUNCTION,
                tags: None,
                deprecated: is_deprecated,
                range,
                selection_range,
                children: param_children_for(full, sv, &f.params),
            })
        }

        StmtKind::Class(c) => {
            let name = c.name.filter(|n| !n.is_error())?;
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
                        ClassMemberKind::Method(m) if !m.name.is_error() => {
                            let mrange = member_range(sv, member);
                            let msel = sv.name_range_after_attrs(
                                &m.name.to_string(),
                                &m.attributes,
                                member.span,
                            );
                            let detail = fn_detail(full, &m.params, m.return_type.as_ref());
                            let method_deprecated = is_deprecated_doc(m.doc_comment.as_ref());
                            vec![DocumentSymbol {
                                name: m.name.to_string(),
                                detail,
                                kind: SymbolKind::METHOD,
                                tags: None,
                                deprecated: method_deprecated,
                                range: mrange,
                                selection_range: msel,
                                children: param_children_for(full, sv, &m.params),
                            }]
                            .into_iter()
                            .chain(promoted_property_symbols_for(sv, &m.params))
                            .collect()
                        }
                        ClassMemberKind::Property(p) if !p.name.is_error() => {
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
                        ClassMemberKind::ClassConst(cc) if !cc.name.is_error() => {
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

        StmtKind::Interface(i) if !i.name.is_error() => {
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&i.name.to_string(), &i.attributes, stmt.span);
            let iface_deprecated = is_deprecated_doc(i.doc_comment.as_ref());
            let children: Vec<DocumentSymbol> = i
                .body
                .members
                .iter()
                .filter_map(|member| match &member.kind {
                    ClassMemberKind::Method(m) if !m.name.is_error() => {
                        let mrange = member_range(sv, member);
                        let msel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        let method_deprecated = is_deprecated_doc(m.doc_comment.as_ref());
                        Some(DocumentSymbol {
                            name: m.name.to_string(),
                            detail: fn_detail(full, &m.params, m.return_type.as_ref()),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: method_deprecated,
                            range: mrange,
                            selection_range: msel,
                            children: None,
                        })
                    }
                    ClassMemberKind::ClassConst(cc) if !cc.name.is_error() => {
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

        StmtKind::Trait(t) if !t.name.is_error() => {
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&t.name.to_string(), &t.attributes, stmt.span);
            let trait_deprecated = is_deprecated_doc(t.doc_comment.as_ref());
            let children: Vec<DocumentSymbol> = t
                .body
                .members
                .iter()
                .filter_map(|member| {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && !m.name.is_error()
                    {
                        let mrange = member_range(sv, member);
                        let msel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        let method_deprecated = is_deprecated_doc(m.doc_comment.as_ref());
                        Some(DocumentSymbol {
                            name: m.name.to_string(),
                            detail: fn_detail(full, &m.params, m.return_type.as_ref()),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: method_deprecated,
                            range: mrange,
                            selection_range: msel,
                            children: param_children_for(full, sv, &m.params),
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

        StmtKind::Enum(e) if !e.name.is_error() => {
            let range = stmt_range(sv, stmt);
            let selection_range =
                sv.name_range_after_attrs(&e.name.to_string(), &e.attributes, stmt.span);
            let enum_deprecated = is_deprecated_doc(e.doc_comment.as_ref());
            let children: Vec<DocumentSymbol> = e
                .body
                .members
                .iter()
                .filter_map(|member| match &member.kind {
                    EnumMemberKind::Case(c) if !c.name.is_error() => {
                        let crange = span_range(sv, member.span);
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
                    EnumMemberKind::Method(m) if !m.name.is_error() => {
                        let mrange = span_range(sv, member.span);
                        let msel = sv.name_range_after_attrs(
                            &m.name.to_string(),
                            &m.attributes,
                            member.span,
                        );
                        let method_deprecated = is_deprecated_doc(m.doc_comment.as_ref());
                        Some(DocumentSymbol {
                            name: m.name.to_string(),
                            detail: fn_detail(full, &m.params, m.return_type.as_ref()),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: method_deprecated,
                            range: mrange,
                            selection_range: msel,
                            children: None,
                        })
                    }
                    EnumMemberKind::ClassConst(cc) if !cc.name.is_error() => {
                        let crange = span_range(sv, member.span);
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
        .filter(|p| !p.name.is_error())
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
) -> Vec<SymbolInformation> {
    let (kind_filter, term) = parse_kind_filter(query);
    let matches_kind = |k: SymbolKind| kind_filter.is_none_or(|f| f == k);
    let fq = crate::text::FuzzyQuery::new(term);

    let mut results = Vec::new();
    for (uri, index) in &wi.files {
        for row in &index.symbols {
            if !matches_kind(row.kind) || !fq.symbol_match(&row.name) {
                continue;
            }
            results.push(SymbolInformation {
                name: row.name.to_string(),
                kind: row.kind,
                location: Location {
                    uri: uri.clone(),
                    range: zero_width_range(row.line),
                },
                tags: None,
                deprecated: row.deprecated,
                container_name: row
                    .container
                    .and_then(|i| index.symbols.get(i as usize))
                    .map(|c| c.name.to_string()),
            });
        }
    }
    results
}

/// Flattens `document_symbols` into the rows `workspace_symbols_from_workspace` matches against.
pub fn workspace_symbol_rows(doc: &ParsedDoc) -> Vec<WorkspaceSymbolRow> {
    let mut rows = Vec::new();
    for sym in outline_symbols(doc) {
        push_workspace_symbol_rows(&sym, None, sym.selection_range.start.line, &mut rows);
    }
    rows
}

fn push_workspace_symbol_rows(
    sym: &DocumentSymbol,
    container: Option<u32>,
    container_line: u32,
    out: &mut Vec<WorkspaceSymbolRow>,
) {
    if sym.kind == SymbolKind::VARIABLE {
        return;
    }
    // Constants and enum cases report their container's line.
    let line = match sym.kind {
        SymbolKind::CONSTANT | SymbolKind::ENUM_MEMBER => container_line,
        _ => sym.selection_range.start.line,
    };
    let own = out.len() as u32;
    out.push(WorkspaceSymbolRow {
        name: sym.name.as_str().into(),
        kind: sym.kind,
        line,
        container,
        deprecated: sym.deprecated,
    });
    for child in sym.children.iter().flatten() {
        push_workspace_symbol_rows(child, Some(own), sym.selection_range.start.line, out);
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

    fn workspace_symbol_dump(src: &str, query: &str) -> String {
        let uri: Uri = "file:///ws/a.php".parse().unwrap();
        let doc = Arc::new(ParsedDoc::parse(src.to_owned()));
        let index = Arc::new(crate::index::file_index::FileIndex::extract(&doc));
        let wi =
            crate::db::workspace_index::WorkspaceIndexData::from_files(vec![(uri.clone(), index)]);
        workspace_symbols_from_workspace(query, &wi)
            .iter()
            .map(|s| {
                format!(
                    "{:?} {} L{} in={:?} dep={:?}",
                    s.kind, s.name, s.location.range.start.line, s.container_name, s.deprecated
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn workspace_symbols_cover_every_declaration_kind() {
        let src = r#"<?php
namespace App {
    #[Attr]
    final class Svc {
        public const A = 1;
        /** @deprecated */
        public int $count = 0;
        public function __construct(private string $name, int $plain) {}
        #[Pure]
        public function run(int $x): void {}
    }
    interface Contract { const B = 2; public function go(): void; }
    trait Helps { public $p; const C = 3; public function help() {} }
    enum Suit: string { case Hearts = 'h'; const D = 4; public function label(): string { return ''; } }
    /** @deprecated */
    function helper($a) {}
    $anon = new class { public function hidden() {} };
}
"#;
        expect_test::expect![[r#"
            Class Svc L3 in=None dep=None
            Constant A L3 in=Some("Svc") dep=None
            Property $count L6 in=Some("Svc") dep=Some(true)
            Method __construct L7 in=Some("Svc") dep=None
            Property $name L7 in=Some("Svc") dep=None
            Method run L9 in=Some("Svc") dep=None
            Interface Contract L11 in=None dep=None
            Constant B L11 in=Some("Contract") dep=None
            Method go L11 in=Some("Contract") dep=None
            Class Helps L12 in=None dep=None
            Method help L12 in=Some("Helps") dep=None
            Enum Suit L13 in=None dep=None
            EnumMember Hearts L13 in=Some("Suit") dep=None
            Constant D L13 in=Some("Suit") dep=None
            Method label L13 in=Some("Suit") dep=None
            Function helper L15 in=None dep=Some(true)"#]]
        .assert_eq(&workspace_symbol_dump(src, ""));
        expect_test::expect![[r#"
            Method __construct L7 in=Some("Svc") dep=None
            Method run L9 in=Some("Svc") dep=None
            Method go L11 in=Some("Contract") dep=None
            Method help L12 in=Some("Helps") dep=None
            Method label L13 in=Some("Suit") dep=None"#]]
        .assert_eq(&workspace_symbol_dump(src, "#method:"));
    }
}
