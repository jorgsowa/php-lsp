use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Param, Stmt, StmtKind};
use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};

use crate::ast::{ParsedDoc, format_type_hint};
use crate::docblock::find_docblock;
use crate::util::word_at;

pub fn hover_info(source: &str, doc: &ParsedDoc, position: Position) -> Option<Hover> {
    let word = word_at(source, position)?;
    let sig = scan_statements(&doc.program().stmts, &word)?;

    let mut value = wrap_php(&sig);
    if let Some(db) = find_docblock(source, &doc.program().stmts, &word) {
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

fn scan_statements(stmts: &[Stmt<'_, '_>], word: &str) -> Option<String> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if f.name == word => {
                let params = format_params(&f.params);
                let ret = f
                    .return_type
                    .as_ref()
                    .map(|r| format!(": {}", format_type_hint(r)))
                    .unwrap_or_default();
                return Some(format!("function {}({}){}", word, params, ret));
            }
            StmtKind::Class(c) if c.name == Some(word) => {
                let mut sig = format!("class {}", word);
                if let Some(ext) = &c.extends {
                    sig.push_str(&format!(" extends {}", ext.to_string_repr()));
                }
                if !c.implements.is_empty() {
                    let ifaces: Vec<String> = c
                        .implements
                        .iter()
                        .map(|i| i.to_string_repr().into_owned())
                        .collect();
                    sig.push_str(&format!(" implements {}", ifaces.join(", ")));
                }
                return Some(sig);
            }
            StmtKind::Interface(i) if i.name == word => {
                return Some(format!("interface {}", word));
            }
            StmtKind::Trait(t) if t.name == word => {
                return Some(format!("trait {}", word));
            }
            StmtKind::Enum(e) if e.name == word => {
                let mut sig = format!("enum {}", word);
                if !e.implements.is_empty() {
                    let ifaces: Vec<String> = e
                        .implements
                        .iter()
                        .map(|i| i.to_string_repr().into_owned())
                        .collect();
                    sig.push_str(&format!(" implements {}", ifaces.join(", ")));
                }
                return Some(sig);
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("function {}({}){}", word, params, ret));
                        }
                    }
                }
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        if m.name == word {
                            let params = format_params(&m.params);
                            let ret = m
                                .return_type
                                .as_ref()
                                .map(|r| format!(": {}", format_type_hint(r)))
                                .unwrap_or_default();
                            return Some(format!("function {}({}){}", word, params, ret));
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(sig) = scan_statements(inner, word) {
                        return Some(sig);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn format_params_str(params: &[Param<'_, '_>]) -> String {
    format_params(params)
}

fn format_params(params: &[Param<'_, '_>]) -> String {
    params
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
        .join(", ")
}

fn wrap_php(sig: &str) -> String {
    format!("```php\n{}\n```", sig)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn hover_on_function_name_returns_signature() {
        let src = "<?php\nfunction greet(string $name): string {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 10));
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 8));
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 2));
        assert!(result.is_none(), "expected None for unknown word");
    }

    #[test]
    fn hover_at_column_beyond_line_length_returns_none() {
        let src = "<?php\nfunction hi() {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 999));
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
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 8));
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 8));
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 8));
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 12));
        assert!(result.is_some(), "expected hover result");
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 10));
        assert!(result.is_some());
        if let Some(Hover {
            contents: HoverContents::Markup(mc),
            ..
        }) = result
        {
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

    #[test]
    fn hover_on_enum_returns_enum_sig() {
        let src = "<?php\nenum Suit {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 6));
        assert!(result.is_some());
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(mc.value.contains("enum Suit"), "expected 'enum Suit', got: {}", mc.value);
        }
    }

    #[test]
    fn hover_on_enum_with_implements_shows_interface() {
        let src = "<?php\nenum Status: string implements Stringable {}";
        let doc = ParsedDoc::parse(src.to_string());
        let result = hover_info(src, &doc, pos(1, 6));
        assert!(result.is_some());
        if let Some(Hover { contents: HoverContents::Markup(mc), .. }) = result {
            assert!(
                mc.value.contains("implements Stringable"),
                "expected implements clause, got: {}",
                mc.value
            );
        }
    }
}
