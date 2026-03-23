use std::sync::Arc;

use php_parser_rs::parser::ast::{
    classes::ClassMember,
    namespaces::NamespaceStatement,
    variables::Variable,
    Expression, Statement,
};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

use crate::type_map::{methods_of_class, TypeMap};

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

pub fn symbol_completions(ast: &[Statement]) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    collect_from_statements(ast, &mut items);
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
                        ClassMember::Property(p) => {
                            for entry in &p.entries {
                                items.push(CompletionItem {
                                    label: entry.variable().name.to_string(),
                                    kind: Some(CompletionItemKind::PROPERTY),
                                    ..Default::default()
                                });
                            }
                        }
                        ClassMember::VariableProperty(p) => {
                            for entry in &p.entries {
                                items.push(CompletionItem {
                                    label: entry.variable().name.to_string(),
                                    kind: Some(CompletionItemKind::PROPERTY),
                                    ..Default::default()
                                });
                            }
                        }
                        ClassMember::Constant(c) => {
                            for entry in &c.entries {
                                items.push(CompletionItem {
                                    label: entry.name.value.to_string(),
                                    kind: Some(CompletionItemKind::CONSTANT),
                                    ..Default::default()
                                });
                            }
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
    ast: &[Statement],
    other_asts: &[Arc<Vec<Statement>>],
    trigger_character: Option<&str>,
) -> Vec<CompletionItem> {
    filtered_completions_at(ast, other_asts, trigger_character, None, None)
}

/// Like `filtered_completions` but also accepts an optional `source` + `position`
/// so that `->` completions can be scoped to the variable's class.
pub fn filtered_completions_at(
    ast: &[Statement],
    other_asts: &[Arc<Vec<Statement>>],
    trigger_character: Option<&str>,
    source: Option<&str>,
    position: Option<Position>,
) -> Vec<CompletionItem> {
    match trigger_character {
        Some("$") => symbol_completions(ast)
            .into_iter()
            .filter(|i| i.kind == Some(CompletionItemKind::VARIABLE))
            .collect(),
        Some(">") => {
            // Try to narrow by the variable before `->` using the type map
            if let (Some(src), Some(pos)) = (source, position) {
                let type_map = TypeMap::from_stmts(ast);
                if let Some(class_name) = resolve_receiver_class(src, pos, &type_map) {
                    // Gather methods from current AST + other ASTs
                    let mut methods: Vec<String> = methods_of_class(ast, &class_name);
                    for other in other_asts {
                        methods.extend(methods_of_class(other, &class_name));
                    }
                    methods.sort();
                    methods.dedup();
                    if !methods.is_empty() {
                        return methods
                            .into_iter()
                            .map(|m| CompletionItem {
                                label: m,
                                kind: Some(CompletionItemKind::METHOD),
                                ..Default::default()
                            })
                            .collect();
                    }
                }
            }
            // Fallback: all methods from AST
            symbol_completions(ast)
                .into_iter()
                .filter(|i| i.kind == Some(CompletionItemKind::METHOD))
                .collect()
        }
        _ => {
            let mut items = keyword_completions();
            items.extend(symbol_completions(ast));
            for other in other_asts {
                let cross: Vec<CompletionItem> = symbol_completions(other)
                    .into_iter()
                    .filter(|i| i.kind != Some(CompletionItemKind::VARIABLE))
                    .collect();
                items.extend(cross);
            }
            let mut seen = std::collections::HashSet::new();
            items.retain(|i| seen.insert(i.label.clone()));
            items
        }
    }
}

/// Given the source and position (the cursor is just after `->`) try to find
/// the variable name that precedes `->` and look up its type.
fn resolve_receiver_class(source: &str, position: Position, type_map: &TypeMap) -> Option<String> {
    let line = source.lines().nth(position.line as usize)?;
    let col = position.character as usize;
    // col is the char index of the trigger `>`. Go back past `->`.
    let arrow_end = col; // position is at the char after `>`
    // Find `->` ending at arrow_end-1 (the `>`)
    let before = &line[..arrow_end.min(line.len())];
    let before = before.strip_suffix("->").unwrap_or(before);
    // Extract the identifier immediately before
    let var_name: String = before
        .chars()
        .rev()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '$')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    if var_name.is_empty() {
        return None;
    }
    let var_name = if var_name.starts_with('$') {
        var_name
    } else {
        format!("${var_name}")
    };
    type_map.get(&var_name).map(|s| s.to_string())
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
        let ast = parse_ast("<?php\nfunction greet() {}");
        let items = symbol_completions(&ast);
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
        let ast = parse_ast("<?php\nclass MyService {}");
        let items = symbol_completions(&ast);
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
        let ast = parse_ast("<?php\nclass Calc { public function add() {} public function sub() {} }");
        let items = symbol_completions(&ast);
        let ls = labels(&items);
        assert!(ls.contains(&"add"), "missing 'add'");
        assert!(ls.contains(&"sub"), "missing 'sub'");
        for item in items.iter().filter(|i| i.label == "add" || i.label == "sub") {
            assert_eq!(item.kind, Some(CompletionItemKind::METHOD));
        }
    }

    #[test]
    fn extracts_function_parameters_as_variables() {
        let ast = parse_ast("<?php\nfunction process($input, $count) {}");
        let items = symbol_completions(&ast);
        let ls = labels(&items);
        assert!(ls.contains(&"$input"), "missing '$input'");
        assert!(ls.contains(&"$count"), "missing '$count'");
    }

    #[test]
    fn extracts_symbols_inside_namespace() {
        let ast = parse_ast("<?php\nnamespace App;\nfunction render() {}\nclass View {}");
        let items = symbol_completions(&ast);
        let ls = labels(&items);
        assert!(ls.contains(&"render"), "missing 'render' in namespaced file");
        assert!(ls.contains(&"View"), "missing 'View' in namespaced file");
    }

    #[test]
    fn extracts_symbols_inside_braced_namespace() {
        let ast = parse_ast("<?php\nnamespace App { function boot() {} class App {} }");
        let items = symbol_completions(&ast);
        let ls = labels(&items);
        assert!(ls.contains(&"boot"), "missing 'boot'");
        assert!(ls.contains(&"App"), "missing 'App'");
    }

    #[test]
    fn partial_ast_used_when_file_has_errors() {
        let ast = parse_ast("<?php\nfunction valid() {}\nclass {");
        let items = symbol_completions(&ast);
        assert!(
            labels(&items).contains(&"valid"),
            "should extract 'valid' even with parse error"
        );
    }

    #[test]
    fn extracts_interface_name() {
        let ast = parse_ast("<?php\ninterface Serializable {}");
        let items = symbol_completions(&ast);
        let item = items.iter().find(|i| i.label == "Serializable");
        assert!(item.is_some(), "missing 'Serializable'");
        assert_eq!(item.unwrap().kind, Some(CompletionItemKind::INTERFACE));
    }

    #[test]
    fn variable_assignment_produces_variable_item() {
        let ast = parse_ast("<?php\n$name = 'Alice';");
        let items = symbol_completions(&ast);
        assert!(
            labels(&items).contains(&"$name"),
            "missing '$name' from assignment"
        );
    }

    #[test]
    fn class_property_appears_in_completions() {
        let ast = parse_ast("<?php\nclass User { public string $name; private int $age; }");
        let items = symbol_completions(&ast);
        let ls = labels(&items);
        assert!(ls.contains(&"$name"), "missing property '$name'");
        assert!(ls.contains(&"$age"), "missing property '$age'");
        for item in items.iter().filter(|i| i.label == "$name" || i.label == "$age") {
            assert_eq!(item.kind, Some(CompletionItemKind::PROPERTY));
        }
    }

    #[test]
    fn class_constant_appears_in_completions() {
        let ast = parse_ast("<?php\nclass Status { const ACTIVE = 1; const INACTIVE = 0; }");
        let items = symbol_completions(&ast);
        let ls = labels(&items);
        assert!(ls.contains(&"ACTIVE"), "missing constant 'ACTIVE'");
        assert!(ls.contains(&"INACTIVE"), "missing constant 'INACTIVE'");
        for item in items.iter().filter(|i| i.label == "ACTIVE" || i.label == "INACTIVE") {
            assert_eq!(item.kind, Some(CompletionItemKind::CONSTANT));
        }
    }

    // --- filtered_completions ---

    #[test]
    fn dollar_trigger_returns_only_variables() {
        let ast = parse_ast("<?php\nfunction greet($name) {}\nclass Foo {}\n$bar = 1;");
        let items = filtered_completions(&ast, &[], Some("$"));
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
        let ast = parse_ast("<?php\nclass Calc { public function add() {} public function sub() {} }");
        let items = filtered_completions(&ast, &[], Some(">"));
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
        let ast = parse_ast("<?php\nfunction greet() {}\nclass MyApp {}");
        let items = filtered_completions(&ast, &[], None);
        let ls = labels(&items);
        assert!(ls.contains(&"function"), "should contain keyword 'function'");
        assert!(ls.contains(&"greet"), "should contain function 'greet'");
        assert!(ls.contains(&"MyApp"), "should contain class 'MyApp'");
    }

    #[test]
    fn dot_trigger_behaves_like_none() {
        let ast = parse_ast("<?php\nfunction greet() {}\nclass MyApp {}");
        let dot_items = filtered_completions(&ast, &[], Some("."));
        let none_items = filtered_completions(&ast, &[], None);
        assert_eq!(
            dot_items.len(),
            none_items.len(),
            "dot trigger should behave like None"
        );
    }

    #[test]
    fn cross_file_symbols_appear_in_default_completions() {
        let ast = parse_ast("<?php\nfunction localFn() {}");
        let other_ast = Arc::new(parse_ast("<?php\nclass RemoteService {}\nfunction remoteHelper() {}"));
        let items = filtered_completions(&ast, &[other_ast], None);
        let ls = labels(&items);
        assert!(ls.contains(&"localFn"), "missing local function");
        assert!(ls.contains(&"RemoteService"), "missing cross-file class");
        assert!(ls.contains(&"remoteHelper"), "missing cross-file function");
    }

    #[test]
    fn cross_file_variables_not_included_in_default_completions() {
        let ast = parse_ast("<?php\n$localVar = 1;");
        let other_ast = Arc::new(parse_ast("<?php\n$remoteVar = 2;"));
        let items = filtered_completions(&ast, &[other_ast], None);
        let ls = labels(&items);
        assert!(!ls.contains(&"$remoteVar"), "cross-file variable should not appear");
    }
}
