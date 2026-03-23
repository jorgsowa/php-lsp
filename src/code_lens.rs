/// `textDocument/codeLens` — inline actionable annotations above declarations.
///
/// Two lens types are emitted:
///   1. **Reference count** — above every function, class, and method declaration.
///   2. **Run test** — above PHPUnit test methods (methods whose name starts with
///      `test` or that carry a `/** @test */` docblock).
use std::sync::Arc;

use php_parser_rs::parser::ast::{
    classes::ClassMember, comments::CommentFormat, namespaces::NamespaceStatement, Statement,
};
use tower_lsp::lsp_types::{
    CodeLens, Command, Position, Range, Url,
};

use crate::diagnostics::span_to_position;
use crate::references::find_references;

/// Build all code lenses for `uri`/`ast`, using `all_docs` for reference counts.
pub fn code_lenses(
    uri: &Url,
    ast: &[Statement],
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> Vec<CodeLens> {
    let mut lenses = Vec::new();
    collect_lenses(ast, uri, all_docs, &mut lenses);
    lenses
}

fn collect_lenses(
    stmts: &[Statement],
    uri: &Url,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
    out: &mut Vec<CodeLens>,
) {
    for stmt in stmts {
        match stmt {
            Statement::Function(f) => {
                let name = f.name.value.to_string();
                let range = name_range(&f.name.span, &name);
                out.push(ref_count_lens(range, &name, all_docs));
            }
            Statement::Class(c) => {
                let class_name = c.name.value.to_string();
                let class_range = name_range(&c.name.span, &class_name);
                out.push(ref_count_lens(class_range, &class_name, all_docs));

                for member in &c.body.members {
                    match member {
                        ClassMember::ConcreteMethod(m) => {
                            let method_name = m.name.value.to_string();
                            let method_range = name_range(&m.name.span, &method_name);
                            out.push(ref_count_lens(method_range, &method_name, all_docs));

                            // Run-test lens for PHPUnit
                            if is_test_method(&method_name, &m.comments) {
                                out.push(run_test_lens(
                                    method_range,
                                    uri,
                                    &class_name,
                                    &method_name,
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Statement::Interface(i) => {
                let name = i.name.value.to_string();
                let range = name_range(&i.name.span, &name);
                out.push(ref_count_lens(range, &name, all_docs));
            }
            Statement::Namespace(ns) => {
                let inner = match ns {
                    NamespaceStatement::Unbraced(u) => &u.statements[..],
                    NamespaceStatement::Braced(b) => &b.body.statements[..],
                };
                collect_lenses(inner, uri, all_docs, out);
            }
            _ => {}
        }
    }
}

// ── Lens constructors ─────────────────────────────────────────────────────────

fn ref_count_lens(
    range: Range,
    name: &str,
    all_docs: &[(Url, Arc<Vec<Statement>>)],
) -> CodeLens {
    let count = find_references(name, all_docs, false).len();
    let label = match count {
        0 => "0 references".to_string(),
        1 => "1 reference".to_string(),
        n => format!("{n} references"),
    };
    CodeLens {
        range,
        command: Some(Command {
            title: label,
            command: "php-lsp.showReferences".to_string(),
            arguments: None,
        }),
        data: None,
    }
}

fn run_test_lens(range: Range, uri: &Url, class: &str, method: &str) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command {
            title: "▶ Run test".to_string(),
            command: "php-lsp.runTest".to_string(),
            arguments: Some(vec![
                serde_json::json!(uri.to_string()),
                serde_json::json!(format!("{class}::{method}")),
            ]),
        }),
        data: None,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn name_range(span: &php_parser_rs::lexer::token::Span, name: &str) -> Range {
    let start = span_to_position(span);
    Range {
        start,
        end: Position {
            line: start.line,
            character: start.character + name.len() as u32,
        },
    }
}

/// A method is a test if its name starts with `test` (PHPUnit convention) or
/// if its leading docblock contains `@test`.
fn is_test_method(
    name: &str,
    comments: &php_parser_rs::parser::ast::comments::CommentGroup,
) -> bool {
    if name.starts_with("test") {
        return true;
    }
    comments.comments.iter().any(|c| {
        c.format == CommentFormat::Document && c.content.to_string().contains("@test")
    })
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

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(path: &str, src: &str) -> (Url, Arc<Vec<Statement>>) {
        (uri(path), Arc::new(parse_ast(src)))
    }

    #[test]
    fn emits_lens_for_top_level_function() {
        let src = "<?php\nfunction greet() {}";
        let ast = parse_ast(src);
        let docs = vec![doc("/a.php", src)];
        let lenses = code_lenses(&uri("/a.php"), &ast, &docs);
        assert!(!lenses.is_empty());
        let titles: Vec<&str> = lenses
            .iter()
            .filter_map(|l| l.command.as_ref())
            .map(|c| c.title.as_str())
            .collect();
        assert!(titles.iter().any(|t| t.ends_with("reference") || t.ends_with("references")));
    }

    #[test]
    fn ref_count_includes_call_sites() {
        let src = "<?php\nfunction greet() {}\ngreet();\ngreet();";
        let ast = parse_ast(src);
        let docs = vec![doc("/a.php", src)];
        let lenses = code_lenses(&uri("/a.php"), &ast, &docs);
        let ref_lens = lenses
            .iter()
            .find(|l| l.command.as_ref().map_or(false, |c| c.title.contains("reference")))
            .unwrap();
        assert!(ref_lens.command.as_ref().unwrap().title.starts_with("2"));
    }

    #[test]
    fn emits_run_test_lens_for_test_method() {
        let src = "<?php\nclass FooTest { public function testSomething() {} }";
        let ast = parse_ast(src);
        let docs = vec![doc("/a.php", src)];
        let lenses = code_lenses(&uri("/a.php"), &ast, &docs);
        let run_test = lenses
            .iter()
            .find(|l| l.command.as_ref().map_or(false, |c| c.title.contains("Run test")));
        assert!(run_test.is_some(), "expected Run test lens");
    }

    #[test]
    fn no_run_test_lens_for_regular_method() {
        let src = "<?php\nclass Foo { public function doWork() {} }";
        let ast = parse_ast(src);
        let docs = vec![doc("/a.php", src)];
        let lenses = code_lenses(&uri("/a.php"), &ast, &docs);
        let run_test = lenses
            .iter()
            .find(|l| l.command.as_ref().map_or(false, |c| c.title.contains("Run test")));
        assert!(run_test.is_none());
    }

    #[test]
    fn emits_lens_for_class_declaration() {
        let src = "<?php\nclass MyService {}";
        let ast = parse_ast(src);
        let docs = vec![doc("/a.php", src)];
        let lenses = code_lenses(&uri("/a.php"), &ast, &docs);
        assert!(!lenses.is_empty());
    }

    #[test]
    fn emits_lens_for_interface() {
        let src = "<?php\ninterface Countable {}";
        let ast = parse_ast(src);
        let docs = vec![doc("/a.php", src)];
        let lenses = code_lenses(&uri("/a.php"), &ast, &docs);
        assert!(!lenses.is_empty());
    }

    #[test]
    fn docblock_test_annotation_triggers_run_test_lens() {
        let src = "<?php\nclass FooTest {\n/** @test */\npublic function it_does_something() {}\n}";
        let ast = parse_ast(src);
        let docs = vec![doc("/a.php", src)];
        let lenses = code_lenses(&uri("/a.php"), &ast, &docs);
        let run_test = lenses
            .iter()
            .find(|l| l.command.as_ref().map_or(false, |c| c.title.contains("Run test")));
        assert!(run_test.is_some(), "expected Run test lens from @test docblock");
    }
}
