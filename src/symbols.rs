#![allow(deprecated)]

use crate::diagnostics::span_to_position;
use php_parser_rs::parser::ast::{
    classes::ClassMember,
    functions::{FunctionParameterList, ReturnType},
    namespaces::NamespaceStatement,
    Statement,
};
use tower_lsp::lsp_types::{DocumentSymbol, Position, Range, SymbolKind};

pub fn document_symbols(source: &str) -> Vec<DocumentSymbol> {
    let program = match php_parser_rs::parser::parse(source) {
        Ok(ast) => ast,
        Err(stack) => stack.partial,
    };
    symbols_from_statements(&program)
}

fn symbols_from_statements(stmts: &[Statement]) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    for stmt in stmts {
        match stmt {
            Statement::Namespace(ns) => match ns {
                NamespaceStatement::Unbraced(u) => {
                    symbols.extend(symbols_from_statements(&u.statements));
                }
                NamespaceStatement::Braced(b) => {
                    symbols.extend(symbols_from_statements(&b.body.statements));
                }
            },
            _ => {
                if let Some(sym) = statement_to_symbol(stmt) {
                    symbols.push(sym);
                }
            }
        }
    }
    symbols
}

fn make_range(start_span: &php_parser_rs::lexer::token::Span, end_span: &php_parser_rs::lexer::token::Span) -> Range {
    let start = span_to_position(start_span);
    let end_pos = span_to_position(end_span);
    Range {
        start,
        end: Position {
            line: end_pos.line,
            character: end_pos.character + 1,
        },
    }
}

fn make_selection_range(name_span: &php_parser_rs::lexer::token::Span, name_len: u32) -> Range {
    let start = span_to_position(name_span);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + name_len,
        },
    }
}

fn format_params(params: &FunctionParameterList) -> String {
    params
        .parameters
        .iter()
        .map(|p| {
            let mut s = String::new();
            if p.ampersand.is_some() {
                s.push('&');
            }
            if p.ellipsis.is_some() {
                s.push_str("...");
            }
            if let Some(t) = &p.data_type {
                s.push_str(&format!("{} ", t));
            }
            s.push_str(&p.name.name.to_string());
            s
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_signature(params: &FunctionParameterList, ret: &Option<ReturnType>) -> String {
    let params_str = format_params(params);
    let ret_str = ret
        .as_ref()
        .map(|r| format!(": {}", r.data_type))
        .unwrap_or_default();
    format!("({}){}", params_str, ret_str)
}

fn statement_to_symbol(stmt: &Statement) -> Option<DocumentSymbol> {
    match stmt {
        Statement::Function(f) => {
            let name = f.name.value.to_string();
            let name_len = name.len() as u32;

            let range = make_range(&f.function, &f.body.right_brace);
            let selection_range = make_selection_range(&f.name.span, name_len);
            let detail = Some(format_signature(&f.parameters, &f.return_type));

            let param_children: Vec<DocumentSymbol> = f
                .parameters
                .parameters
                .iter()
                .map(|p| {
                    let pname = p.name.name.to_string();
                    let plen = pname.len() as u32;
                    let psel = make_selection_range(&p.name.span, plen);
                    DocumentSymbol {
                        name: pname,
                        detail: None,
                        kind: SymbolKind::VARIABLE,
                        tags: None,
                        deprecated: None,
                        range: psel,
                        selection_range: psel,
                        children: None,
                    }
                })
                .collect();

            Some(DocumentSymbol {
                name,
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

        Statement::Class(c) => {
            let name = c.name.value.to_string();
            let name_len = name.len() as u32;

            let range = make_range(&c.class, &c.body.right_brace);
            let selection_range = make_selection_range(&c.name.span, name_len);

            let method_children: Vec<DocumentSymbol> = c
                .body
                .members
                .iter()
                .filter_map(|member| match member {
                    ClassMember::ConcreteMethod(m) => {
                        let mname = m.name.value.to_string();
                        let mlen = mname.len() as u32;
                        let mrange = make_range(&m.function, &m.body.right_brace);
                        let msel = make_selection_range(&m.name.span, mlen);
                        Some(DocumentSymbol {
                            name: mname,
                            detail: Some(format_signature(&m.parameters, &m.return_type)),
                            kind: SymbolKind::METHOD,
                            tags: None,
                            deprecated: None,
                            range: mrange,
                            selection_range: msel,
                            children: None,
                        })
                    }
                    ClassMember::AbstractMethod(m) => {
                        let mname = m.name.value.to_string();
                        let mlen = mname.len() as u32;
                        let msel = make_selection_range(&m.name.span, mlen);
                        let mstart = span_to_position(&m.function);
                        let msemi = span_to_position(&m.semicolon);
                        let mrange = Range {
                            start: mstart,
                            end: Position {
                                line: msemi.line,
                                character: msemi.character + 1,
                            },
                        };
                        Some(DocumentSymbol {
                            name: mname,
                            detail: Some(format_signature(&m.parameters, &m.return_type)),
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
                name,
                detail: None,
                kind: SymbolKind::CLASS,
                tags: None,
                deprecated: None,
                range,
                selection_range,
                children: if method_children.is_empty() {
                    None
                } else {
                    Some(method_children)
                },
            })
        }

        Statement::Interface(i) => {
            let name = i.name.value.to_string();
            let name_len = name.len() as u32;

            let range = make_range(&i.interface, &i.body.right_brace);
            let selection_range = make_selection_range(&i.name.span, name_len);

            Some(DocumentSymbol {
                name,
                detail: None,
                kind: SymbolKind::INTERFACE,
                tags: None,
                deprecated: None,
                range,
                selection_range,
                children: None,
            })
        }

        Statement::Trait(t) => {
            let name = t.name.value.to_string();
            let name_len = name.len() as u32;

            let range = make_range(&t.r#trait, &t.body.right_brace);
            let selection_range = make_selection_range(&t.name.span, name_len);

            Some(DocumentSymbol {
                name,
                detail: None,
                kind: SymbolKind::CLASS,
                tags: None,
                deprecated: None,
                range,
                selection_range,
                children: None,
            })
        }

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_has_function_kind_and_signature_detail() {
        let src = "<?php\nfunction greet(string $name): string {}";
        let syms = document_symbols(src);
        let f = syms.iter().find(|s| s.name == "greet").expect("greet not found");
        assert_eq!(f.kind, SymbolKind::FUNCTION);
        let detail = f.detail.as_deref().unwrap_or("");
        assert!(detail.contains("$name"), "detail should contain '$name', got: {detail}");
        assert!(detail.contains(": string"), "detail should contain return type, got: {detail}");
    }

    #[test]
    fn function_parameters_are_variable_children() {
        let src = "<?php\nfunction process($input, $count) {}";
        let syms = document_symbols(src);
        let f = syms.iter().find(|s| s.name == "process").expect("process not found");
        let children = f.children.as_ref().expect("should have children");
        assert!(
            children.iter().any(|c| c.name == "$input" && c.kind == SymbolKind::VARIABLE),
            "missing $input child"
        );
        assert!(
            children.iter().any(|c| c.name == "$count" && c.kind == SymbolKind::VARIABLE),
            "missing $count child"
        );
    }

    #[test]
    fn class_has_class_kind_with_method_children() {
        let src = "<?php\nclass Calc { public function add() {} public function sub() {} }";
        let syms = document_symbols(src);
        let c = syms.iter().find(|s| s.name == "Calc").expect("Calc not found");
        assert_eq!(c.kind, SymbolKind::CLASS);
        let children = c.children.as_ref().expect("should have method children");
        assert!(
            children.iter().any(|m| m.name == "add" && m.kind == SymbolKind::METHOD),
            "missing 'add' method"
        );
        assert!(
            children.iter().any(|m| m.name == "sub" && m.kind == SymbolKind::METHOD),
            "missing 'sub' method"
        );
    }

    #[test]
    fn interface_has_interface_kind() {
        let src = "<?php\ninterface Serializable {}";
        let syms = document_symbols(src);
        let i = syms.iter().find(|s| s.name == "Serializable").expect("Serializable not found");
        assert_eq!(i.kind, SymbolKind::INTERFACE);
    }

    #[test]
    fn trait_has_class_kind() {
        let src = "<?php\ntrait Loggable {}";
        let syms = document_symbols(src);
        let t = syms.iter().find(|s| s.name == "Loggable").expect("Loggable not found");
        assert_eq!(t.kind, SymbolKind::CLASS);
    }

    #[test]
    fn symbols_inside_namespace_are_returned() {
        let src = "<?php\nnamespace App;\nfunction render() {}\nclass View {}";
        let syms = document_symbols(src);
        assert!(syms.iter().any(|s| s.name == "render"), "missing 'render'");
        assert!(syms.iter().any(|s| s.name == "View"), "missing 'View'");
    }

    #[test]
    fn range_start_lte_selection_range_start() {
        let src = "<?php\nfunction hello(string $x): int {}";
        let syms = document_symbols(src);
        let f = syms.iter().find(|s| s.name == "hello").expect("hello not found");
        // full range starts at `function` keyword, selection at name — same line, range.start <= sel
        assert!(
            f.range.start.line <= f.selection_range.start.line,
            "range.start.line should be <= selection_range.start.line"
        );
        if f.range.start.line == f.selection_range.start.line {
            assert!(
                f.range.start.character <= f.selection_range.start.character,
                "range.start.character should be <= selection_range.start.character"
            );
        }
    }

    #[test]
    fn partial_ast_on_parse_error_returns_valid_symbols() {
        let src = "<?php\nfunction valid() {}\nclass {";
        let syms = document_symbols(src);
        assert!(
            syms.iter().any(|s| s.name == "valid"),
            "should still return 'valid' despite parse error"
        );
    }
}
