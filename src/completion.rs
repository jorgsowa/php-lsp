use php_parser_rs::parser::ast::{
    classes::ClassMember,
    namespaces::NamespaceStatement,
    variables::Variable,
    Expression, Statement,
};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind};

const PHP_KEYWORDS: &[&str] = &[
    "abstract", "and", "array", "as", "break", "callable", "case", "catch", "class", "clone",
    "const", "continue", "declare", "default", "die", "do", "echo", "else", "elseif", "empty",
    "enddeclare", "endfor", "endforeach", "endif", "endswitch", "endwhile", "enum", "eval",
    "exit", "extends", "final", "finally", "fn", "for", "foreach", "function", "global", "goto",
    "if", "implements", "include", "include_once", "instanceof", "insteadof", "interface",
    "isset", "list", "match", "namespace", "new", "null", "or", "print", "private", "protected",
    "public", "readonly", "require", "require_once", "return", "self", "static", "switch",
    "throw", "trait", "true", "false", "try", "use", "var", "while", "xor", "yield",
];

pub fn keyword_completions() -> Vec<CompletionItem> {
    PHP_KEYWORDS
        .iter()
        .map(|kw| CompletionItem {
            label: kw.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect()
}

pub fn symbol_completions(source: &str) -> Vec<CompletionItem> {
    let program = match php_parser_rs::parser::parse(source) {
        Ok(ast) => ast,
        Err(stack) => stack.partial,
    };

    let mut items = Vec::new();
    collect_from_statements(&program, &mut items);
    items
}

fn collect_from_statements(stmts: &[Statement], items: &mut Vec<CompletionItem>) {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) => {
                items.push(CompletionItem {
                    label: f.name.value.to_string(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    ..Default::default()
                });
                for param in f.parameters.parameters.iter() {
                    items.push(CompletionItem {
                        // ByteString already includes the leading `$`
                        label: param.name.name.to_string(),
                        kind: Some(CompletionItemKind::VARIABLE),
                        ..Default::default()
                    });
                }
            }
            Statement::Class(c) => {
                items.push(CompletionItem {
                    label: c.name.value.to_string(),
                    kind: Some(CompletionItemKind::CLASS),
                    ..Default::default()
                });
                for member in c.body.members.iter() {
                    match member {
                        ClassMember::ConcreteMethod(m) => {
                            items.push(CompletionItem {
                                label: m.name.value.to_string(),
                                kind: Some(CompletionItemKind::METHOD),
                                ..Default::default()
                            });
                        }
                        ClassMember::AbstractMethod(m) => {
                            items.push(CompletionItem {
                                label: m.name.value.to_string(),
                                kind: Some(CompletionItemKind::METHOD),
                                ..Default::default()
                            });
                        }
                        ClassMember::ConcreteConstructor(_) => {
                            items.push(CompletionItem {
                                label: "__construct".to_string(),
                                kind: Some(CompletionItemKind::CONSTRUCTOR),
                                ..Default::default()
                            });
                        }
                        _ => {}
                    }
                }
            }
            Statement::Interface(i) => {
                items.push(CompletionItem {
                    label: i.name.value.to_string(),
                    kind: Some(CompletionItemKind::INTERFACE),
                    ..Default::default()
                });
            }
            Statement::Trait(t) => {
                items.push(CompletionItem {
                    label: t.name.value.to_string(),
                    kind: Some(CompletionItemKind::CLASS),
                    ..Default::default()
                });
            }
            Statement::Namespace(ns) => match ns {
                NamespaceStatement::Unbraced(u) => {
                    collect_from_statements(&u.statements, items);
                }
                NamespaceStatement::Braced(b) => {
                    collect_from_statements(&b.body.statements, items);
                }
            },
            Statement::Expression(e) => {
                collect_from_expression(&e.expression, items);
            }
            _ => {}
        }
    }
}

fn collect_from_expression(expr: &Expression, items: &mut Vec<CompletionItem>) {
    match expr {
        Expression::AssignmentOperation(assign) => {
            if let Expression::Variable(Variable::SimpleVariable(v)) = assign.left() {
                let name = v.name.to_string();
                // ByteString already includes the leading `$`; skip $this
                if name != "$this" {
                    items.push(CompletionItem {
                        label: name,
                        kind: Some(CompletionItemKind::VARIABLE),
                        ..Default::default()
                    });
                }
            }
            collect_from_expression(assign.right(), items);
        }
        _ => {}
    }
}

pub fn filtered_completions(
    source: &str,
    trigger_character: Option<&str>,
) -> Vec<CompletionItem> {
    match trigger_character {
        Some("$") => symbol_completions(source)
            .into_iter()
            .filter(|i| i.kind == Some(CompletionItemKind::VARIABLE))
            .collect(),
        Some(">") => symbol_completions(source)
            .into_iter()
            .filter(|i| i.kind == Some(CompletionItemKind::METHOD))
            .collect(),
        _ => {
            let mut items = keyword_completions();
            items.extend(symbol_completions(source));
            items
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    // --- keyword_completions ---

    #[test]
    fn keywords_list_is_non_empty() {
        assert!(!keyword_completions().is_empty());
    }

    #[test]
    fn keywords_contain_common_php_keywords() {
        let kws = keyword_completions();
        let labels = labels(&kws);
        for expected in &["function", "class", "return", "foreach", "match", "namespace"] {
            assert!(labels.contains(expected), "missing keyword: {expected}");
        }
    }

    #[test]
    fn all_keyword_items_have_keyword_kind() {
        for item in keyword_completions() {
            assert_eq!(item.kind, Some(CompletionItemKind::KEYWORD));
        }
    }

    // --- symbol_completions ---

    #[test]
    fn extracts_top_level_function_name() {
        let src = "<?php\nfunction greet() {}";
        let items = symbol_completions(src);
        assert!(
            labels(&items).contains(&"greet"),
            "expected 'greet' in {:?}",
            labels(&items)
        );
        let greet = items.iter().find(|i| i.label == "greet").unwrap();
        assert_eq!(greet.kind, Some(CompletionItemKind::FUNCTION));
    }

    #[test]
    fn extracts_top_level_class_name() {
        let src = "<?php\nclass MyService {}";
        let items = symbol_completions(src);
        assert!(
            labels(&items).contains(&"MyService"),
            "expected 'MyService' in {:?}",
            labels(&items)
        );
        let cls = items.iter().find(|i| i.label == "MyService").unwrap();
        assert_eq!(cls.kind, Some(CompletionItemKind::CLASS));
    }

    #[test]
    fn extracts_class_method_names() {
        let src = "<?php\nclass Calc { public function add() {} public function sub() {} }";
        let items = symbol_completions(src);
        let ls = labels(&items);
        assert!(ls.contains(&"add"), "missing 'add'");
        assert!(ls.contains(&"sub"), "missing 'sub'");
        for item in items.iter().filter(|i| i.label == "add" || i.label == "sub") {
            assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        }
    }

    #[test]
    fn extracts_function_parameters_as_variables() {
        let src = "<?php\nfunction process($input, $count) {}";
        let items = symbol_completions(src);
        let ls = labels(&items);
        assert!(ls.contains(&"$input"), "missing '$input'");
        assert!(ls.contains(&"$count"), "missing '$count'");
    }

    #[test]
    fn extracts_symbols_inside_namespace() {
        let src = "<?php\nnamespace App;\nfunction render() {}\nclass View {}";
        let items = symbol_completions(src);
        let ls = labels(&items);
        assert!(ls.contains(&"render"), "missing 'render' in namespaced file");
        assert!(ls.contains(&"View"), "missing 'View' in namespaced file");
    }

    #[test]
    fn extracts_symbols_inside_braced_namespace() {
        let src = "<?php\nnamespace App { function boot() {} class App {} }";
        let items = symbol_completions(src);
        let ls = labels(&items);
        assert!(ls.contains(&"boot"), "missing 'boot'");
        assert!(ls.contains(&"App"), "missing 'App'");
    }

    #[test]
    fn partial_ast_used_when_file_has_errors() {
        // Incomplete file: function is defined but class declaration is broken
        let src = "<?php\nfunction valid() {}\nclass {";
        let items = symbol_completions(src);
        assert!(
            labels(&items).contains(&"valid"),
            "should extract 'valid' even with parse error"
        );
    }

    #[test]
    fn extracts_interface_name() {
        let src = "<?php\ninterface Serializable {}";
        let items = symbol_completions(src);
        let item = items.iter().find(|i| i.label == "Serializable");
        assert!(item.is_some(), "missing 'Serializable'");
        assert_eq!(item.unwrap().kind, Some(CompletionItemKind::INTERFACE));
    }

    #[test]
    fn variable_assignment_produces_variable_item() {
        let src = "<?php\n$name = 'Alice';";
        let items = symbol_completions(src);
        assert!(
            labels(&items).contains(&"$name"),
            "missing '$name' from assignment"
        );
    }

    // --- filtered_completions ---

    #[test]
    fn dollar_trigger_returns_only_variables() {
        let src = "<?php\nfunction greet($name) {}\nclass Foo {}\n$bar = 1;";
        let items = filtered_completions(src, Some("$"));
        assert!(!items.is_empty(), "should have variable items");
        for item in &items {
            assert_eq!(
                item.kind,
                Some(CompletionItemKind::VARIABLE),
                "expected VARIABLE kind, got {:?} for '{}'",
                item.kind,
                item.label
            );
        }
        let ls = labels(&items);
        assert!(!ls.contains(&"greet"), "should not contain function");
        assert!(!ls.contains(&"Foo"), "should not contain class");
    }

    #[test]
    fn arrow_trigger_returns_only_methods() {
        let src = "<?php\nclass Calc { public function add() {} public function sub() {} }";
        let items = filtered_completions(src, Some(">"));
        assert!(!items.is_empty(), "should have method items");
        for item in &items {
            assert_eq!(
                item.kind,
                Some(CompletionItemKind::METHOD),
                "expected METHOD kind, got {:?} for '{}'",
                item.kind,
                item.label
            );
        }
    }

    #[test]
    fn none_trigger_returns_keywords_functions_classes() {
        let src = "<?php\nfunction greet() {}\nclass MyApp {}";
        let items = filtered_completions(src, None);
        let ls = labels(&items);
        assert!(ls.contains(&"function"), "should contain keyword 'function'");
        assert!(ls.contains(&"greet"), "should contain function 'greet'");
        assert!(ls.contains(&"MyApp"), "should contain class 'MyApp'");
    }

    #[test]
    fn dot_trigger_behaves_like_none() {
        let src = "<?php\nfunction greet() {}\nclass MyApp {}";
        let dot_items = filtered_completions(src, Some("."));
        let none_items = filtered_completions(src, None);
        assert_eq!(
            dot_items.len(),
            none_items.len(),
            "dot trigger should behave like None"
        );
    }
}
