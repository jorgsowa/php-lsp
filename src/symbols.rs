#![allow(deprecated)]

use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    DocumentSymbol, Location, OneOf, Position, Range, SymbolInformation, SymbolKind, Url,
    WorkspaceSymbol,
};

use crate::ast::{ParsedDoc, SourceView, name_range};
use crate::docblock::{docblock_before, parse_docblock};

pub fn document_symbols(_source: &str, doc: &ParsedDoc) -> Vec<DocumentSymbol> {
    let sv = doc.view();
    symbols_from_statements(sv, &doc.program().stmts)
}

/// Fill in the sv.source() range for a `WorkspaceSymbol` whose `location` carries only a URI
/// (i.e. `OneOf::Right(WorkspaceLocation)`).  If the range is already present, or if the
/// document cannot be found, the symbol is returned unchanged.
pub fn resolve_workspace_symbol(
    mut symbol: WorkspaceSymbol,
    docs: &[(Url, Arc<ParsedDoc>)],
) -> WorkspaceSymbol {
    let uri = match &symbol.location {
        // Already fully resolved — nothing to do.
        OneOf::Left(_) => return symbol,
        OneOf::Right(wl) => wl.uri.clone(),
    };
    for (doc_uri, doc) in docs {
        if doc_uri == &uri {
            let range = name_range(doc.source(), doc.line_starts(), &symbol.name);
            symbol.location = OneOf::Left(Location { uri, range });
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
                    symbols.extend(symbols_from_statements(sv, inner));
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

fn statement_to_symbol(sv: SourceView<'_>, stmt: &Stmt<'_, '_>) -> Option<DocumentSymbol> {
    match &stmt.kind {
        StmtKind::Function(f) => {
            let range = stmt_range(sv, stmt);
            let selection_range = sv.name_range(f.name);
            let detail = Some(format_fn_signature(&f.params, f.return_type.as_ref()));
            let is_deprecated = docblock_before(sv.source(), stmt.span.start)
                .filter(|raw| parse_docblock(raw).deprecated.is_some())
                .map(|_| true);

            let param_children: Vec<DocumentSymbol> = f
                .params
                .iter()
                .map(|p| {
                    let prange = param_range(sv, p);
                    let psel = sv.name_range(p.name);
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

            Some(DocumentSymbol {
                name: f.name.to_string(),
                detail,
                kind: SymbolKind::FUNCTION,
                tags: None,
                deprecated: is_deprecated,
                range,
                selection_range,
                children: if param_children.is_empty() {
                    None
                } else {
                    Some(param_children)
                },
            })
        }

        StmtKind::Class(c) => {
            let name = c.name?;
            let range = stmt_range(sv, stmt);
            let selection_range = sv.name_range(name);
            let class_deprecated = docblock_before(sv.source(), stmt.span.start)
                .filter(|raw| parse_docblock(raw).deprecated.is_some())
                .map(|_| true);

            let children: Vec<DocumentSymbol> = c
                .members
                .iter()
                .flat_map(|member| -> Vec<DocumentSymbol> {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mrange = member_range(sv, member);
                            let msel = sv.name_range(m.name);
                            let detail =
                                Some(format_fn_signature(&m.params, m.return_type.as_ref()));
                            let method_deprecated = docblock_before(sv.source(), member.span.start)
                                .filter(|raw| parse_docblock(raw).deprecated.is_some())
                                .map(|_| true);
                            vec![DocumentSymbol {
                                name: m.name.to_string(),
                                detail,
                                kind: SymbolKind::METHOD,
                                tags: None,
                                deprecated: method_deprecated,
                                range: mrange,
                                selection_range: msel,
                                children: None,
                            }]
                        }
                        ClassMemberKind::Property(p) => {
                            let prange = member_range(sv, member);
                            let psel = sv.name_range(p.name);
                            let prop_deprecated = docblock_before(sv.source(), member.span.start)
                                .filter(|raw| parse_docblock(raw).deprecated.is_some())
                                .map(|_| true);
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
                            let csel = sv.name_range(cc.name);
                            let const_deprecated = docblock_before(sv.source(), member.span.start)
                                .filter(|raw| parse_docblock(raw).deprecated.is_some())
                                .map(|_| true);
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
            let selection_range = sv.name_range(i.name);
            let iface_deprecated = docblock_before(sv.source(), stmt.span.start)
                .filter(|raw| parse_docblock(raw).deprecated.is_some())
                .map(|_| true);
            let children: Vec<DocumentSymbol> = i
                .members
                .iter()
                .filter_map(|member| {
                    if let ClassMemberKind::ClassConst(cc) = &member.kind {
                        let crange = member_range(sv, member);
                        let csel = sv.name_range(cc.name);
                        let const_deprecated = docblock_before(sv.source(), member.span.start)
                            .filter(|raw| parse_docblock(raw).deprecated.is_some())
                            .map(|_| true);
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
                    } else {
                        None
                    }
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
            let selection_range = sv.name_range(t.name);
            let trait_deprecated = docblock_before(sv.source(), stmt.span.start)
                .filter(|raw| parse_docblock(raw).deprecated.is_some())
                .map(|_| true);
            let children: Vec<DocumentSymbol> = t
                .members
                .iter()
                .filter_map(|member| {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let mrange = member_range(sv, member);
                        let msel = sv.name_range(m.name);
                        let method_deprecated = docblock_before(sv.source(), member.span.start)
                            .filter(|raw| parse_docblock(raw).deprecated.is_some())
                            .map(|_| true);
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
            let selection_range = sv.name_range(e.name);
            let enum_deprecated = docblock_before(sv.source(), stmt.span.start)
                .filter(|raw| parse_docblock(raw).deprecated.is_some())
                .map(|_| true);
            let children: Vec<DocumentSymbol> = e
                .members
                .iter()
                .filter_map(|member| match &member.kind {
                    EnumMemberKind::Case(c) => {
                        let crange = Range {
                            start: sv.position_of(member.span.start),
                            end: sv.position_of(member.span.end),
                        };
                        let csel = sv.name_range(c.name);
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
                        let msel = sv.name_range(m.name);
                        let method_deprecated = docblock_before(sv.source(), member.span.start)
                            .filter(|raw| parse_docblock(raw).deprecated.is_some())
                            .map(|_| true);
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
    use crate::ast::format_type_hint;
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

fn _pos_from_offset(sv: SourceView<'_>, offset: u32) -> Position {
    sv.position_of(offset)
}

// ── Index-based variants ──────────────────────────────────────────────────────

/// `workspace_symbols` variant that queries `FileIndex` entries instead of
/// full `ParsedDoc` ASTs.  Used by the backend for cross-file symbol search
/// when background files only retain a compact index.
#[allow(deprecated)]
pub fn workspace_symbols_from_index(
    query: &str,
    indexes: &[(Url, Arc<crate::file_index::FileIndex>)],
) -> Vec<SymbolInformation> {
    use crate::file_index::ClassKind;
    use crate::util::fuzzy_camel_match;

    let (kind_filter, term) = parse_kind_filter(query);
    let matches_kind = |k: SymbolKind| kind_filter.is_none_or(|f| f == k);

    let line_range = |line: u32| -> Range {
        let pos = Position { line, character: 0 };
        Range {
            start: pos,
            end: pos,
        }
    };

    let mut results = Vec::new();
    for (uri, idx) in indexes {
        if matches_kind(SymbolKind::FUNCTION) {
            for f in &idx.functions {
                if fuzzy_camel_match(term, &f.name) {
                    results.push(SymbolInformation {
                        name: f.name.clone(),
                        kind: SymbolKind::FUNCTION,
                        location: Location {
                            uri: uri.clone(),
                            range: line_range(f.start_line),
                        },
                        tags: None,
                        deprecated: None,
                        container_name: None,
                    });
                }
            }
        }
        for cls in &idx.classes {
            let class_kind = match cls.kind {
                ClassKind::Class | ClassKind::Trait => SymbolKind::CLASS,
                ClassKind::Interface => SymbolKind::INTERFACE,
                ClassKind::Enum => SymbolKind::ENUM,
            };
            if matches_kind(class_kind) && fuzzy_camel_match(term, &cls.name) {
                results.push(SymbolInformation {
                    name: cls.name.clone(),
                    kind: class_kind,
                    location: Location {
                        uri: uri.clone(),
                        range: line_range(cls.start_line),
                    },
                    tags: None,
                    deprecated: None,
                    container_name: None,
                });
            }
            if matches_kind(SymbolKind::METHOD) {
                for m in &cls.methods {
                    if fuzzy_camel_match(term, &m.name) {
                        results.push(SymbolInformation {
                            name: m.name.clone(),
                            kind: SymbolKind::METHOD,
                            location: Location {
                                uri: uri.clone(),
                                range: line_range(m.start_line),
                            },
                            tags: None,
                            deprecated: None,
                            container_name: Some(cls.name.clone()),
                        });
                    }
                }
            }
            if matches_kind(SymbolKind::ENUM_MEMBER) && cls.kind == ClassKind::Enum {
                for case in &cls.cases {
                    if fuzzy_camel_match(term, case) {
                        results.push(SymbolInformation {
                            name: case.clone(),
                            kind: SymbolKind::ENUM_MEMBER,
                            location: Location {
                                uri: uri.clone(),
                                range: line_range(cls.start_line),
                            },
                            tags: None,
                            deprecated: None,
                            container_name: Some(cls.name.clone()),
                        });
                    }
                }
            }
        }
    }
    results
}

/// Phase J — Thin wrapper over `workspace_symbols_from_index` that reads the
/// `(Url, Arc<FileIndex>)` list out of the salsa-memoized aggregate. The
/// inner walk is unchanged (fuzzy match is inherently O(total symbols)); the
/// win is that every handler shares the same aggregate `Arc`, rebuilt only on
/// edits, instead of each request rebuilding the list via `all_indexes()`
/// (which takes the host mutex once per file).
pub fn workspace_symbols_from_workspace(
    query: &str,
    wi: &crate::db::workspace_index::WorkspaceIndexData,
) -> Vec<SymbolInformation> {
    workspace_symbols_from_index(query, &wi.files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_has_function_kind_and_signature_detail() {
        let src = "<?php\nfunction greet(string $name): string {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let f = syms
            .iter()
            .find(|s| s.name == "greet")
            .expect("greet not found");
        assert_eq!(f.kind, SymbolKind::FUNCTION);
        let detail = f.detail.as_deref().unwrap_or("");
        assert!(
            detail.contains("$name"),
            "detail should contain '$name', got: {detail}"
        );
        assert!(
            detail.contains(": string"),
            "detail should contain return type, got: {detail}"
        );
    }

    #[test]
    fn function_parameters_are_variable_children() {
        let src = "<?php\nfunction process($input, $count) {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let f = syms
            .iter()
            .find(|s| s.name == "process")
            .expect("process not found");
        let children = f.children.as_ref().expect("should have children");
        assert!(
            children
                .iter()
                .any(|c| c.name == "$input" && c.kind == SymbolKind::VARIABLE),
            "missing $input child"
        );
        assert!(
            children
                .iter()
                .any(|c| c.name == "$count" && c.kind == SymbolKind::VARIABLE),
            "missing $count child"
        );
    }

    #[test]
    fn class_has_class_kind_with_method_children() {
        let src = "<?php\nclass Calc { public function add() {} public function sub() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let c = syms
            .iter()
            .find(|s| s.name == "Calc")
            .expect("Calc not found");
        assert_eq!(c.kind, SymbolKind::CLASS);
        let children = c.children.as_ref().expect("should have method children");
        assert!(
            children
                .iter()
                .any(|m| m.name == "add" && m.kind == SymbolKind::METHOD)
        );
        assert!(
            children
                .iter()
                .any(|m| m.name == "sub" && m.kind == SymbolKind::METHOD)
        );
    }

    #[test]
    fn interface_has_interface_kind() {
        let src = "<?php\ninterface Serializable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let i = syms
            .iter()
            .find(|s| s.name == "Serializable")
            .expect("Serializable not found");
        assert_eq!(i.kind, SymbolKind::INTERFACE);
    }

    #[test]
    fn trait_has_class_kind() {
        let src = "<?php\ntrait Loggable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let t = syms
            .iter()
            .find(|s| s.name == "Loggable")
            .expect("Loggable not found");
        assert_eq!(t.kind, SymbolKind::CLASS);
    }

    #[test]
    fn symbols_inside_namespace_are_returned() {
        let src = "<?php\nnamespace App {\nfunction render() {}\nclass View {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        assert!(syms.iter().any(|s| s.name == "render"), "missing 'render'");
        assert!(syms.iter().any(|s| s.name == "View"), "missing 'View'");
    }

    #[test]
    fn range_start_lte_selection_range_start() {
        let src = "<?php\nfunction hello(string $x): int {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let f = syms
            .iter()
            .find(|s| s.name == "hello")
            .expect("hello not found");
        assert!(f.range.start.line <= f.selection_range.start.line);
        if f.range.start.line == f.selection_range.start.line {
            assert!(f.range.start.character <= f.selection_range.start.character);
        }
    }

    #[test]
    fn class_properties_are_property_children() {
        let src = "<?php\nclass User { public string $name; private int $age; }";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let c = syms
            .iter()
            .find(|s| s.name == "User")
            .expect("User not found");
        let children = c.children.as_ref().expect("should have children");
        assert!(
            children
                .iter()
                .any(|ch| ch.name == "$name" && ch.kind == SymbolKind::PROPERTY)
        );
        assert!(
            children
                .iter()
                .any(|ch| ch.name == "$age" && ch.kind == SymbolKind::PROPERTY)
        );
    }

    #[test]
    fn class_constants_are_constant_children() {
        let src = "<?php\nclass Status { const ACTIVE = 1; const INACTIVE = 0; }";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let c = syms
            .iter()
            .find(|s| s.name == "Status")
            .expect("Status not found");
        let children = c.children.as_ref().expect("should have children");
        assert!(
            children
                .iter()
                .any(|ch| ch.name == "ACTIVE" && ch.kind == SymbolKind::CONSTANT)
        );
        assert!(
            children
                .iter()
                .any(|ch| ch.name == "INACTIVE" && ch.kind == SymbolKind::CONSTANT)
        );
    }

    #[test]
    fn trait_methods_are_method_children() {
        let src = "<?php\ntrait Loggable { public function log() {} public function warn() {} }";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let t = syms
            .iter()
            .find(|s| s.name == "Loggable")
            .expect("Loggable not found");
        let children = t
            .children
            .as_ref()
            .expect("trait should have method children");
        assert!(
            children
                .iter()
                .any(|m| m.name == "log" && m.kind == SymbolKind::METHOD)
        );
        assert!(
            children
                .iter()
                .any(|m| m.name == "warn" && m.kind == SymbolKind::METHOD)
        );
    }

    #[test]
    fn partial_ast_on_parse_error_returns_valid_symbols() {
        let src = "<?php\nfunction valid() {}\nclass {";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        assert!(
            syms.iter().any(|s| s.name == "valid"),
            "should still return 'valid' despite parse error"
        );
    }

    #[test]
    fn function_symbol_has_correct_range() {
        // The symbol range should start at the line where the `function` keyword is.
        // Source: line 0 = "<?php", line 1 = "function myFunc() {}"
        let src = "<?php\nfunction myFunc() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let f = syms
            .iter()
            .find(|s| s.name == "myFunc")
            .expect("myFunc not found");
        assert_eq!(
            f.kind,
            SymbolKind::FUNCTION,
            "symbol should have FUNCTION kind"
        );
        assert_eq!(
            f.range.start.line, 1,
            "function range should start at line 1 (where 'function' keyword is)"
        );
        // The selection_range (name range) should also be on line 1.
        assert_eq!(
            f.selection_range.start.line, 1,
            "selection_range should start at line 1"
        );
    }

    #[test]
    fn enum_symbol_has_correct_kind() {
        // An enum declaration should produce a symbol with SymbolKind::ENUM.
        let src = "<?php\nenum Color { case Red; case Green; case Blue; }";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let e = syms
            .iter()
            .find(|s| s.name == "Color")
            .expect("Color enum not found");
        assert_eq!(
            e.kind,
            SymbolKind::ENUM,
            "enum should produce a symbol with SymbolKind::ENUM"
        );
        assert_eq!(e.range.start.line, 1, "enum range should start at line 1");
    }

    #[test]
    fn interface_constants_are_constant_children() {
        // Interface constants should appear as CONSTANT children in document symbols.
        let src =
            "<?php\ninterface Config {\n    const VERSION = '1.0';\n    const DEBUG = false;\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let i = syms
            .iter()
            .find(|s| s.name == "Config")
            .expect("Config interface not found");
        let children = i
            .children
            .as_ref()
            .expect("interface should have constant children");
        assert!(
            children
                .iter()
                .any(|c| c.name == "VERSION" && c.kind == SymbolKind::CONSTANT),
            "missing VERSION constant child, got: {:?}",
            children.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        assert!(
            children
                .iter()
                .any(|c| c.name == "DEBUG" && c.kind == SymbolKind::CONSTANT),
            "missing DEBUG constant child"
        );
        assert_eq!(
            children.len(),
            2,
            "expected exactly 2 constant children, got: {:?}",
            children.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn interface_without_constants_has_no_children() {
        // An interface with only abstract methods (no constants) should have children: None.
        let src = "<?php\ninterface Runnable {\n    public function run(): void;\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let i = syms
            .iter()
            .find(|s| s.name == "Runnable")
            .expect("Runnable not found");
        assert!(
            i.children.is_none(),
            "interface with no constants should have no children, got: {:?}",
            i.children
        );
    }

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

    #[test]
    fn deprecated_function_sets_deprecated_field() {
        let src = "<?php\n/** @deprecated Use newGreet() instead */\nfunction greet() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let f = syms
            .iter()
            .find(|s| s.name == "greet")
            .expect("greet not found");
        assert_eq!(
            f.deprecated,
            Some(true),
            "deprecated function should have deprecated: Some(true)"
        );
    }

    #[test]
    fn non_deprecated_function_has_no_deprecated_field() {
        let src = "<?php\n/** Does stuff. */\nfunction greet() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let f = syms
            .iter()
            .find(|s| s.name == "greet")
            .expect("greet not found");
        assert_eq!(
            f.deprecated, None,
            "non-deprecated function should have deprecated: None"
        );
    }

    #[test]
    fn deprecated_class_sets_deprecated_field() {
        let src = "<?php\n/** @deprecated */\nclass OldService {}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let c = syms
            .iter()
            .find(|s| s.name == "OldService")
            .expect("OldService not found");
        assert_eq!(
            c.deprecated,
            Some(true),
            "deprecated class should have deprecated: Some(true)"
        );
    }

    #[test]
    fn deprecated_method_sets_deprecated_field() {
        let src =
            "<?php\nclass Svc {\n    /** @deprecated */\n    public function oldMethod() {}\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let syms = document_symbols(src, &doc);
        let c = syms
            .iter()
            .find(|s| s.name == "Svc")
            .expect("Svc not found");
        let children = c.children.as_ref().expect("Svc should have children");
        let m = children
            .iter()
            .find(|ch| ch.name == "oldMethod")
            .expect("oldMethod not found");
        assert_eq!(
            m.deprecated,
            Some(true),
            "deprecated method should have deprecated: Some(true)"
        );
    }
}
