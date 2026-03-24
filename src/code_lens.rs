/// `textDocument/codeLens` — inline actionable annotations above declarations.
///
/// Two lens types are emitted:
///   1. **Reference count** — above every function, class, and method declaration.
///   2. **Run test** — above PHPUnit test methods (methods whose name starts with
///      `test` or that carry a `/** @test */` docblock).
use std::sync::Arc;

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{CodeLens, Command, Url};

use crate::ast::{ParsedDoc, name_range};
use crate::docblock::docblock_before;
use crate::references::find_references;

/// Build all code lenses for `uri`/`doc`, using `all_docs` for reference counts.
pub fn code_lenses(
    uri: &Url,
    doc: &ParsedDoc,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<CodeLens> {
    let source = doc.source();
    let mut lenses = Vec::new();
    collect_lenses(&doc.program().stmts, source, uri, all_docs, &mut lenses);
    lenses
}

fn collect_lenses(
    stmts: &[Stmt<'_, '_>],
    source: &str,
    uri: &Url,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    out: &mut Vec<CodeLens>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let range = name_range(source, f.name);
                out.push(ref_count_lens(range, f.name, all_docs));
            }
            StmtKind::Class(c) => {
                if let Some(class_name) = c.name {
                    let class_range = name_range(source, class_name);
                    out.push(ref_count_lens(class_range, class_name, all_docs));

                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let method_range = name_range(source, m.name);
                            out.push(ref_count_lens(method_range, m.name, all_docs));

                            if is_test_method(source, m.name, member.span.start) {
                                out.push(run_test_lens(method_range, uri, class_name, m.name));
                            }
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                let range = name_range(source, i.name);
                out.push(ref_count_lens(range, i.name, all_docs));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_lenses(inner, source, uri, all_docs, out);
                }
            }
            _ => {}
        }
    }
}

// ── Lens constructors ─────────────────────────────────────────────────────────

fn ref_count_lens(
    range: tower_lsp::lsp_types::Range,
    name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
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

fn run_test_lens(
    range: tower_lsp::lsp_types::Range,
    uri: &Url,
    class: &str,
    method: &str,
) -> CodeLens {
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

/// A method is a test if its name starts with `test` (PHPUnit convention) or
/// if its leading docblock contains `@test`.
fn is_test_method(source: &str, name: &str, member_start: u32) -> bool {
    if name.starts_with("test") {
        return true;
    }
    docblock_before(source, member_start)
        .map(|db| db.contains("@test"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    #[test]
    fn emits_lens_for_top_level_function() {
        let src = "<?php\nfunction greet() {}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        assert!(!lenses.is_empty());
        let titles: Vec<&str> = lenses
            .iter()
            .filter_map(|l| l.command.as_ref())
            .map(|c| c.title.as_str())
            .collect();
        assert!(
            titles
                .iter()
                .any(|t| t.ends_with("reference") || t.ends_with("references"))
        );
    }

    #[test]
    fn ref_count_includes_call_sites() {
        let src = "<?php\nfunction greet() {}\ngreet();\ngreet();";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let ref_lens = lenses
            .iter()
            .find(|l| {
                l.command
                    .as_ref()
                    .map_or(false, |c| c.title.contains("reference"))
            })
            .unwrap();
        assert!(ref_lens.command.as_ref().unwrap().title.starts_with("2"));
    }

    #[test]
    fn emits_run_test_lens_for_test_method() {
        let src = "<?php\nclass FooTest { public function testSomething() {} }";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let run_test = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("Run test"))
        });
        assert!(run_test.is_some(), "expected Run test lens");
    }

    #[test]
    fn no_run_test_lens_for_regular_method() {
        let src = "<?php\nclass Foo { public function doWork() {} }";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let run_test = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("Run test"))
        });
        assert!(run_test.is_none());
    }

    #[test]
    fn emits_lens_for_class_declaration() {
        let src = "<?php\nclass MyService {}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        assert!(!lenses.is_empty());
    }

    #[test]
    fn emits_lens_for_interface() {
        let src = "<?php\ninterface Countable {}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        assert!(!lenses.is_empty());
    }

    #[test]
    fn docblock_test_annotation_triggers_run_test_lens() {
        let src = "<?php\nclass FooTest {\n/** @test */\npublic function it_does_something() {}\n}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let run_test = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("Run test"))
        });
        assert!(
            run_test.is_some(),
            "expected Run test lens from @test docblock"
        );
    }
}
