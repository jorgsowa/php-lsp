/// `textDocument/codeLens` — inline actionable annotations above declarations.
///
/// Two lens types are emitted:
///   1. **Reference count** — above every function, class, and method declaration.
///   2. **Run test** — above PHPUnit test methods (methods whose name starts with
///      `test` or that carry a `/** @test */` docblock).
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{CodeLens, Command, Url};

use crate::ast::{ParsedDoc, name_range};
use crate::docblock::docblock_before;
use crate::implementation::find_implementations;
use crate::references::find_references;
use crate::type_map::{members_of_class, parent_class_name};

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

                    // Implementations count for abstract classes (classes extending this).
                    if c.modifiers.is_abstract {
                        let impl_count = find_implementations(class_name, all_docs).len();
                        out.push(impl_count_lens(class_range, impl_count));
                    }

                    // Find the parent class once for the whole class.
                    let parent = find_parent_class(c, all_docs);

                    for member in c.members.iter() {
                        if let ClassMemberKind::Method(m) = &member.kind {
                            let method_range = name_range(source, m.name);
                            out.push(ref_count_lens(method_range, m.name, all_docs));

                            if is_test_method(source, m.name, member.span.start) {
                                out.push(run_test_lens(method_range, uri, class_name, m.name));
                            }

                            // Overrides lens: show if parent class has a method with the same name.
                            if let Some(ref parent_name) = parent {
                                if parent_has_method(parent_name, m.name, all_docs) {
                                    out.push(overrides_lens(method_range, parent_name, m.name));
                                }
                            }
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                let range = name_range(source, i.name);
                out.push(ref_count_lens(range, i.name, all_docs));
                // Implementations count lens.
                let impl_count = find_implementations(i.name, all_docs).len();
                out.push(impl_count_lens(range, impl_count));
            }
            StmtKind::Trait(t) => {
                let range = name_range(source, t.name);
                out.push(ref_count_lens(range, t.name, all_docs));
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let method_range = name_range(source, m.name);
                        out.push(ref_count_lens(method_range, m.name, all_docs));
                    }
                }
            }
            StmtKind::Enum(e) => {
                let range = name_range(source, e.name);
                out.push(ref_count_lens(range, e.name, all_docs));
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        let method_range = name_range(source, m.name);
                        out.push(ref_count_lens(method_range, m.name, all_docs));
                    }
                }
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

fn impl_count_lens(range: tower_lsp::lsp_types::Range, count: usize) -> CodeLens {
    let label = match count {
        0 => "0 implementations".to_string(),
        1 => "1 implementation".to_string(),
        n => format!("{n} implementations"),
    };
    CodeLens {
        range,
        command: Some(Command {
            title: label,
            command: "php-lsp.showImplementations".to_string(),
            arguments: None,
        }),
        data: None,
    }
}

fn overrides_lens(
    range: tower_lsp::lsp_types::Range,
    parent_class: &str,
    method_name: &str,
) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command {
            title: format!("overrides {}::{}", parent_class, method_name),
            command: "php-lsp.goToDeclaration".to_string(),
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

/// Return the direct parent class name of a class, if any.
fn find_parent_class<'a>(
    c: &php_ast::ClassDecl<'_, '_>,
    all_docs: &'a [(Url, Arc<ParsedDoc>)],
) -> Option<String> {
    let parent_short = c.extends.as_ref()?.to_string_repr().into_owned();
    // Resolve through the documents to get the canonical short name.
    for (_, doc) in all_docs {
        if let Some(p) = parent_class_name(doc, &parent_short) {
            return Some(p);
        }
    }
    Some(parent_short)
}

/// Check whether `parent_class` declares a method named `method_name`.
fn parent_has_method(parent_class: &str, method_name: &str, all_docs: &[(Url, Arc<ParsedDoc>)]) -> bool {
    for (_, doc) in all_docs {
        let members = members_of_class(doc, parent_class);
        if members.methods.iter().any(|(n, _)| n == method_name) {
            return true;
        }
    }
    false
}

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
        assert_eq!(lenses.len(), 1, "expected exactly 1 lens for a top-level function");
        let cmd = lenses[0].command.as_ref().expect("lens should have a command");
        // No callers -> "0 references"
        assert_eq!(cmd.title, "0 references", "unused function should show '0 references'");
        assert_eq!(
            cmd.command, "php-lsp.showReferences",
            "command name should be 'php-lsp.showReferences'"
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
                .map_or(false, |c| c.command == "php-lsp.runTest")
        });
        assert!(run_test.is_some(), "expected Run test lens");
        let cmd = run_test.unwrap().command.as_ref().unwrap();
        assert_eq!(cmd.command, "php-lsp.runTest", "command name must be 'php-lsp.runTest'");
        assert!(
            cmd.title.contains("Run test"),
            "title should contain 'Run test', got: {}",
            cmd.title
        );
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
        assert_eq!(lenses.len(), 1, "expected exactly 1 lens for a class declaration");
        let cmd = lenses[0].command.as_ref().expect("lens should have a command");
        assert_eq!(cmd.title, "0 references", "unused class should show '0 references'");
    }

    #[test]
    fn emits_lens_for_interface() {
        let src = "<?php\ninterface Countable {}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        // Interface gets a ref-count lens + an implementations lens.
        assert_eq!(lenses.len(), 2, "expected 2 lenses (ref-count + implementations) for interface");
        let titles: Vec<&str> = lenses
            .iter()
            .filter_map(|l| l.command.as_ref())
            .map(|c| c.title.as_str())
            .collect();
        assert!(
            titles.iter().any(|t| t.ends_with("reference") || t.ends_with("references")),
            "one lens should be a reference count, got: {:?}", titles
        );
        assert!(
            titles.iter().any(|t| t.contains("implementation")),
            "one lens should be an implementations count, got: {:?}", titles
        );
    }

    #[test]
    fn emits_implementations_lens_for_interface() {
        let src = "<?php\ninterface Countable {}\nclass MyList implements Countable {}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let impl_lens = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("implementation"))
        });
        assert!(impl_lens.is_some(), "expected implementations lens");
        assert!(
            impl_lens
                .unwrap()
                .command
                .as_ref()
                .unwrap()
                .title
                .starts_with("1"),
            "expected 1 implementation"
        );
    }

    #[test]
    fn emits_implementations_lens_for_abstract_class() {
        let src = "<?php\nabstract class Shape {}\nclass Circle extends Shape {}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let impl_lens = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("implementation"))
        });
        assert!(impl_lens.is_some(), "expected implementations lens on abstract class");
    }

    #[test]
    fn emits_overrides_lens_for_overriding_method() {
        let src = "<?php\nclass Base { public function run(): void {} }\nclass Child extends Base { public function run(): void {} }";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let overrides = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("overrides"))
        });
        assert!(overrides.is_some(), "expected overrides lens");
        assert!(
            overrides
                .unwrap()
                .command
                .as_ref()
                .unwrap()
                .title
                .contains("Base::run"),
            "overrides lens should reference Base::run"
        );
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

    #[test]
    fn ref_count_lens_shows_zero_for_unused() {
        // A function with no call sites should show "0 references".
        let src = "<?php\nfunction unusedFn() {}";
        let d = doc(src);
        // Use only this single doc so there are no call sites.
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let ref_lens = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.command == "php-lsp.showReferences")
        });
        let cmd = ref_lens
            .expect("expected a showReferences lens")
            .command
            .as_ref()
            .unwrap();
        assert_eq!(
            cmd.title,
            "0 references",
            "function with no callers should show '0 references', got: {}",
            cmd.title
        );
    }

    #[test]
    fn run_test_lens_has_correct_command() {
        // The Run test lens must use command "php-lsp.runTest" and title "▶ Run test".
        let src = "<?php\nclass SomeTest { public function testItWorks() {} }";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let run_test_lens = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.command == "php-lsp.runTest")
        });
        let cmd = run_test_lens
            .expect("expected a php-lsp.runTest lens")
            .command
            .as_ref()
            .unwrap();
        assert_eq!(
            cmd.command,
            "php-lsp.runTest",
            "command name must be exactly 'php-lsp.runTest'"
        );
        assert_eq!(
            cmd.title,
            "▶ Run test",
            "title must be exactly '▶ Run test', got: {}",
            cmd.title
        );
    }

    #[test]
    fn emits_lens_for_enum_declaration() {
        let src = "<?php\nenum Suit { case Hearts; }";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        assert!(
            lenses.iter().any(|l| l
                .command
                .as_ref()
                .map_or(false, |c| c.title.contains("reference"))),
            "expected a ref-count lens for enum declaration"
        );
    }

    #[test]
    fn emits_lens_for_trait_declaration() {
        let src = "<?php\ntrait Loggable { public function log(): void {} }";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        assert!(
            lenses.iter().any(|l| l
                .command
                .as_ref()
                .map_or(false, |c| c.title.contains("reference"))),
            "expected a ref-count lens for trait declaration"
        );
    }

    #[test]
    fn emits_lens_for_enum_method() {
        let src = "<?php\nenum Suit { public function label(): string { return 'x'; } }";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        // Should have at least 2 lenses: one for the enum itself, one for the method.
        assert!(
            lenses.len() >= 2,
            "expected lenses for both enum and enum method, got {} lens(es)",
            lenses.len()
        );
    }
}
