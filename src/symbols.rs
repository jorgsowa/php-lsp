#![allow(deprecated)]

use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    DocumentSymbol, Location, OneOf, Position, Range, SymbolInformation, SymbolKind, Url,
    WorkspaceSymbol,
};

use crate::ast::{ParsedDoc, name_range, offset_to_position};
use crate::util::fuzzy_camel_match;

pub fn document_symbols(source: &str, doc: &ParsedDoc) -> Vec<DocumentSymbol> {
    symbols_from_statements(source, &doc.program().stmts)
}

/// Fill in the source range for a `WorkspaceSymbol` whose `location` carries only a URI
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
            let range = name_range(doc.source(), &symbol.name);
            symbol.location = OneOf::Left(Location { uri, range });
            break;
        }
    }
    symbol
}

/// Flat symbol search across all open documents.
/// Matches by camel/underscore abbreviation or plain case-insensitive substring.
pub fn workspace_symbols(query: &str, docs: &[(Url, Arc<ParsedDoc>)]) -> Vec<SymbolInformation> {
    let mut results = Vec::new();
    for (uri, doc) in docs {
        let source = doc.source();
        collect_symbol_info(source, &doc.program().stmts, query, uri, &mut results);
    }
    results
}

#[allow(deprecated)]
fn collect_symbol_info(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    query: &str,
    uri: &Url,
    out: &mut Vec<SymbolInformation>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let name = f.name;
                if fuzzy_camel_match(query, name) {
                    out.push(SymbolInformation {
                        name: name.to_string(),
                        kind: SymbolKind::FUNCTION,
                        location: Location {
                            uri: uri.clone(),
                            range: name_range(source, name),
                        },
                        tags: None,
                        deprecated: None,
                        container_name: None,
                    });
                }
            }
            StmtKind::Class(c) => {
                let name = c.name.unwrap_or("");
                if !name.is_empty() && fuzzy_camel_match(query, name) {
                    out.push(SymbolInformation {
                        name: name.to_string(),
                        kind: SymbolKind::CLASS,
                        location: Location {
                            uri: uri.clone(),
                            range: name_range(source, name),
                        },
                        tags: None,
                        deprecated: None,
                        container_name: None,
                    });
                }
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if fuzzy_camel_match(query, m.name) {
                            out.push(SymbolInformation {
                                name: m.name.to_string(),
                                kind: SymbolKind::METHOD,
                                location: Location {
                                    uri: uri.clone(),
                                    range: name_range(source, m.name),
                                },
                                tags: None,
                                deprecated: None,
                                container_name: if !name.is_empty() {
                                    Some(name.to_string())
                                } else {
                                    None
                                },
                            });
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                if fuzzy_camel_match(query, i.name) {
                    out.push(SymbolInformation {
                        name: i.name.to_string(),
                        kind: SymbolKind::INTERFACE,
                        location: Location {
                            uri: uri.clone(),
                            range: name_range(source, i.name),
                        },
                        tags: None,
                        deprecated: None,
                        container_name: None,
                    });
                }
            }
            StmtKind::Trait(t) => {
                if fuzzy_camel_match(query, t.name) {
                    out.push(SymbolInformation {
                        name: t.name.to_string(),
                        kind: SymbolKind::CLASS,
                        location: Location {
                            uri: uri.clone(),
                            range: name_range(source, t.name),
                        },
                        tags: None,
                        deprecated: None,
                        container_name: None,
                    });
                }
            }
            StmtKind::Enum(e) => {
                if fuzzy_camel_match(query, e.name) {
                    out.push(SymbolInformation {
                        name: e.name.to_string(),
                        kind: SymbolKind::ENUM,
                        location: Location {
                            uri: uri.clone(),
                            range: name_range(source, e.name),
                        },
                        tags: None,
                        deprecated: None,
                        container_name: None,
                    });
                }
                for member in e.members.iter() {
                    if let EnumMemberKind::Case(c) = &member.kind {
                        if fuzzy_camel_match(query, c.name) {
                            out.push(SymbolInformation {
                                name: c.name.to_string(),
                                kind: SymbolKind::ENUM_MEMBER,
                                location: Location {
                                    uri: uri.clone(),
                                    range: name_range(source, c.name),
                                },
                                tags: None,
                                deprecated: None,
                                container_name: Some(e.name.to_string()),
                            });
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_symbol_info(source, inner, query, uri, out);
                }
            }
            _ => {}
        }
    }
}

fn symbols_from_statements(source: &str, stmts: &[Stmt<'_, '_>]) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    symbols.extend(symbols_from_statements(source, inner));
                }
            }
            _ => {
                if let Some(sym) = statement_to_symbol(source, stmt) {
                    symbols.push(sym);
                }
            }
        }
    }
    symbols
}

fn stmt_range(source: &str, stmt: &Stmt<'_, '_>) -> Range {
    let start = offset_to_position(source, stmt.span.start);
    let end = offset_to_position(source, stmt.span.end);
    Range { start, end }
}

fn member_range(source: &str, member: &php_ast::ClassMember<'_, '_>) -> Range {
    let start = offset_to_position(source, member.span.start);
    let end = offset_to_position(source, member.span.end);
    Range { start, end }
}

fn param_range(source: &str, param: &php_ast::Param<'_, '_>) -> Range {
    let start = offset_to_position(source, param.span.start);
    let end = offset_to_position(source, param.span.end);
    Range { start, end }
}

fn statement_to_symbol(source: &str, stmt: &Stmt<'_, '_>) -> Option<DocumentSymbol> {
    match &stmt.kind {
        StmtKind::Function(f) => {
            let range = stmt_range(source, stmt);
            let selection_range = name_range(source, f.name);
            let detail = Some(format_fn_signature(&f.params, f.return_type.as_ref()));

            let param_children: Vec<DocumentSymbol> = f
                .params
                .iter()
                .map(|p| {
                    let prange = param_range(source, p);
                    let psel = name_range(source, p.name);
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
                deprecated: None,
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
            let range = stmt_range(source, stmt);
            let selection_range = name_range(source, name);

            let children: Vec<DocumentSymbol> = c
                .members
                .iter()
                .flat_map(|member| -> Vec<DocumentSymbol> {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let mrange = member_range(source, member);
                            let msel = name_range(source, m.name);
                            let detail =
                                Some(format_fn_signature(&m.params, m.return_type.as_ref()));
                            vec![DocumentSymbol {
                                name: m.name.to_string(),
                                detail,
                                kind: SymbolKind::METHOD,
                                tags: None,
                                deprecated: None,
                                range: mrange,
                                selection_range: msel,
                                children: None,
                            }]
                        }
                        ClassMemberKind::Property(p) => {
                            let prange = member_range(source, member);
                            let psel = name_range(source, p.name);
                            vec![DocumentSymbol {
                                name: format!("${}", p.name),
                                detail: None,
                                kind: SymbolKind::PROPERTY,
                                tags: None,
                                deprecated: None,
                                range: prange,
                                selection_range: psel,
                                children: None,
                            }]
                        }
                        ClassMemberKind::ClassConst(cc) => {
                            let crange = member_range(source, member);
                            let csel = name_range(source, cc.name);
                            vec![DocumentSymbol {
                                name: cc.name.to_string(),
                                detail: None,
                                kind: SymbolKind::CONSTANT,
                                tags: None,
                                deprecated: None,
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
                deprecated: None,
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
            let range = stmt_range(source, stmt);
            let selection_range = name_range(source, i.name);
            let children: Vec<DocumentSymbol> = i
                .members
                .iter()
                .filter_map(|member| {
                    if let ClassMemberKind::ClassConst(cc) = &member.kind {
                        let crange = member_range(source, member);
                        let csel = name_range(source, cc.name);
                        Some(DocumentSymbol {
                            name: cc.name.to_string(),
                            detail: None,
                            kind: SymbolKind::CONSTANT,
                            tags: None,
                            deprecated: None,
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
                deprecated: None,
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
            let range = stmt_range(source, stmt);
            let selection_range = name_range(source, t.name);
            let children: Vec<DocumentSymbol> = t
                .members
                .iter()
                .filter_map(|member| {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let mrange = member_range(source, member);
                        let msel = name_range(source, m.name);
                        Some(DocumentSymbol {
                            name: m.name.to_string(),
                            detail: Some(format_fn_signature(&m.params, m.return_type.as_ref())),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: None,
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
                deprecated: None,
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
            let range = stmt_range(source, stmt);
            let selection_range = name_range(source, e.name);
            let children: Vec<DocumentSymbol> = e
                .members
                .iter()
                .filter_map(|member| match &member.kind {
                    EnumMemberKind::Case(c) => {
                        let crange = Range {
                            start: offset_to_position(source, member.span.start),
                            end: offset_to_position(source, member.span.end),
                        };
                        let csel = name_range(source, c.name);
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
                            start: offset_to_position(source, member.span.start),
                            end: offset_to_position(source, member.span.end),
                        };
                        let msel = name_range(source, m.name);
                        Some(DocumentSymbol {
                            name: m.name.to_string(),
                            detail: Some(format_fn_signature(&m.params, m.return_type.as_ref())),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: None,
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
                deprecated: None,
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

fn _pos_from_offset(source: &str, offset: u32) -> Position {
    offset_to_position(source, offset)
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
}
