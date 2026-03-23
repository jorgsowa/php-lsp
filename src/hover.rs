use php_parser_rs::parser::ast::{classes::ClassMember, namespaces::NamespaceStatement, Statement};
use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::docblock::find_docblock;
use crate::util::word_at;

pub fn hover_info(source: &str, ast: &[Statement], position: Position) -> Option<Hover> {
    let word = word_at(source, position)?;
    let sig = scan_statements(ast, &word)?;

    // Build the hover value: PHP code block + optional docblock below
    let mut value = wrap_php(&sig);
    if let Some(db) = find_docblock(ast, &word) {
        let md = db.to_markdown();
        if !md.is_empty() {
            value.push_str("\n\n---\n\n");
            value.push_str(&md);
        }
    }

    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: None,
    })
}

fn scan_statements(stmts: &[Statement], word: &str) -> Option<String> {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) if f.name.value.to_string() == word => {
                let params = format_params(&f.parameters);
                let ret = f
                    .return_type
                    .as_ref()
                    .map(|r| format!(": {}", r.data_type))
                    .unwrap_or_default();
                return Some(format!("function {}({}){}", word, params, ret));
            }
            Statement::Class(c) if c.name.value.to_string() == word => {
                let mut sig = format!("class {}", word);
                if let Some(ext) = &c.extends {
                    sig.push_str(&format!(" extends {}", ext.parent));
                }
                if let Some(imp) = &c.implements {
                    let ifaces: Vec<String> =
                        imp.interfaces.iter().map(|i| i.value.to_string()).collect();
                    sig.push_str(&format!(" implements {}", ifaces.join(", ")));
                }
                return Some(sig);
            }
            Statement::Interface(i) if i.name.value.to_string() == word => {
                return Some(format!("interface {}", word));
            }
            Statement::Trait(t) if t.name.value.to_string() == word => {
                return Some(format!("trait {}", word));
            }
            Statement::Class(c) => {
                for member in &c.body.members {
                    match member {
                        ClassMember::ConcreteMethod(m) if m.name.value.to_string() == word => {
                            let params = format_params(&m.parameters);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", r.data_type))
                                .unwrap_or_default();
                            return Some(format!("function {}({}){}", word, params, ret));
                        }
                        _ => {}
                    }
                }
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => scan_statements(&u.statements, word),
                    NamespaceStatement::Braced(b) => scan_statements(&b.body.statements, word),
                };
                if inner.is_some() {
                    return inner;
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn format_params_str(params: &php_parser_rs::parser::ast::functions::FunctionParameterList) -> String {
    format_params(params)
}

fn format_params(params: &php_parser_rs::parser::ast::functions::FunctionParameterList) -> String {
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

fn wrap_php(sig: &str) -> String {
    format!("```php\n{}\n```", sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ast(source: &str) -> Vec<Statement> {
        match php_parser_rs::parser::parse(source) {
            Ok(ast) => ast,
            Err(stack) => stack.partial,
        }
    }

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn hover_on_function_name_returns_signature() {
        let src = "<?php\nfunction greet(string $name): string {}";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 10));
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("function greet("),
                "expected function signature, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_class_name_returns_class_sig() {
        let src = "<?php\nclass MyService {}";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 8));
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("class MyService"),
                "expected class sig, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_unknown_word_returns_none() {
        let src = "<?php\n$unknown = 42;";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 2));
        assert!(result.is_none(), "expected None for unknown word");
    }

    #[test]
    fn hover_at_column_beyond_line_length_returns_none() {
        let src = "<?php\nfunction hi() {}";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 999));
        assert!(result.is_none());
    }

    #[test]
    fn word_at_extracts_from_middle_of_identifier() {
        let src = "<?php\nfunction greetUser() {}";
        let word = word_at(src, pos(1, 13));
        assert_eq!(word.as_deref(), Some("greetUser"));
    }

    #[test]
    fn hover_on_class_with_extends_shows_parent() {
        let src = "<?php\nclass Dog extends Animal {}";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 8));
        assert!(result.is_some());
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("extends Animal"),
                "expected 'extends Animal', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_class_with_implements_shows_interfaces() {
        let src = "<?php\nclass Repo implements Countable, Serializable {}";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 8));
        assert!(result.is_some());
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("implements Countable, Serializable"),
                "expected implements list, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_trait_returns_trait_sig() {
        let src = "<?php\ntrait Loggable {}";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 8));
        assert!(result.is_some());
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("trait Loggable"),
                "expected 'trait Loggable', got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn hover_on_interface_returns_interface_sig() {
        let src = "<?php\ninterface Serializable {}";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 12));
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("interface Serializable"),
                "expected interface sig, got: {}",
                mc.value
            );
        }
    }

    #[test]
    fn function_with_no_params_no_return_shows_no_colon() {
        let src = "<?php\nfunction init() {}";
        let ast = parse_ast(src);
        let result = hover_info(src, &ast, pos(1, 10));
        assert!(result.is_some());
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("function init()"),
                "expected 'function init()', got: {}",
                mc.value
            );
            assert!(
                !mc.value.contains(':'),
                "should not contain ':' when no return type, got: {}",
                mc.value
            );
        }
    }
}
