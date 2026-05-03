/// `textDocument/codeLens` — inline actionable annotations above declarations.
///
/// Four lens types are emitted:
///   1. **Reference count** — above every function, class, and method declaration.
///   2. **Run test** — above PHPUnit test methods (methods whose name starts with
///      `test` or that carry a `/** @test */` docblock).
///   3. **N implementations** — above abstract classes, interfaces, and traits,
///      counting classes that extend/implement/use them.
///   4. **overrides ClassName::method** — above methods that override a parent
///      class method of the same name.
use std::sync::Arc;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use serde_json::json;
use tower_lsp::lsp_types::{CodeLens, Command, Url};

use crate::ast::{ParsedDoc, SourceView};
use crate::docblock::docblock_before;
use crate::implementation::find_implementations;
use crate::references::{SymbolKind, find_references};
use crate::type_map::parent_class_name;

/// Build all code lenses for `uri`/`doc`, using `all_docs` for reference counts.
pub fn code_lenses(
    uri: &Url,
    doc: &ParsedDoc,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<CodeLens> {
    let sv = doc.view();
    let mut lenses = Vec::new();
    collect_lenses(&doc.program().stmts, sv, uri, all_docs, &mut lenses);
    lenses
}

fn collect_lenses(
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    uri: &Url,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    out: &mut Vec<CodeLens>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let range = sv.name_range(f.name);
                out.push(ref_count_lens(range, f.name, uri, all_docs, None));
            }
            StmtKind::Class(c) => {
                if let Some(class_name) = c.name {
                    let class_range = sv.name_range(class_name);
                    out.push(ref_count_lens(class_range, class_name, uri, all_docs, None));

                    // Implementations count for abstract classes (classes extending this).
                    if c.modifiers.is_abstract {
                        let impls = find_implementations(class_name, None, all_docs);
                        out.push(impl_count_lens(class_range, uri, impls));
                    }

                    // Direct supertypes — extends parent + used traits — checked once
                    // per class for overrides lookups on each method.
                    let parents = collect_direct_supertypes(c, all_docs);

                    for member in c.members.iter() {
                        match &member.kind {
                            ClassMemberKind::Method(m) => {
                                let method_range = sv.name_range(m.name);
                                out.push(ref_count_lens(method_range, m.name, uri, all_docs, None));

                                if is_test_method(sv.source(), m, member.span.start) {
                                    out.push(run_test_lens(method_range, uri, class_name, m.name));
                                }

                                // Overrides lens: emit for each direct supertype (parent class
                                // or used trait) that declares a method with the same name.
                                for parent_name in &parents {
                                    if let Some(parent_loc) =
                                        parent_method_location(parent_name, m.name, all_docs)
                                    {
                                        out.push(overrides_lens(
                                            method_range,
                                            uri,
                                            parent_name,
                                            m.name,
                                            parent_loc,
                                        ));
                                    }
                                }

                                // Constructor-promoted params: `public function __construct(public string $name)`.
                                if m.name == "__construct" {
                                    for p in m.params.iter() {
                                        if p.visibility.is_some() {
                                            let prop_range = sv.name_range(p.name);
                                            out.push(ref_count_lens(
                                                prop_range,
                                                p.name,
                                                uri,
                                                all_docs,
                                                Some(SymbolKind::Property),
                                            ));
                                        }
                                    }
                                }
                            }
                            ClassMemberKind::Property(p) => {
                                let prop_range = sv.name_range(p.name);
                                out.push(ref_count_lens(
                                    prop_range,
                                    p.name,
                                    uri,
                                    all_docs,
                                    Some(SymbolKind::Property),
                                ));
                            }
                            _ => {}
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                let range = sv.name_range(i.name);
                out.push(ref_count_lens(range, i.name, uri, all_docs, None));
                // Implementations count lens.
                let impls = find_implementations(i.name, None, all_docs);
                out.push(impl_count_lens(range, uri, impls));
            }
            StmtKind::Trait(t) => {
                let range = sv.name_range(t.name);
                out.push(ref_count_lens(range, t.name, uri, all_docs, None));
                // Usages: classes that `use` this trait.
                let usages = trait_usage_locations(t.name, all_docs);
                out.push(impl_count_lens(range, uri, usages));
                for member in t.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let method_range = sv.name_range(m.name);
                            out.push(ref_count_lens(method_range, m.name, uri, all_docs, None));
                        }
                        ClassMemberKind::Property(p) => {
                            let prop_range = sv.name_range(p.name);
                            out.push(ref_count_lens(
                                prop_range,
                                p.name,
                                uri,
                                all_docs,
                                Some(SymbolKind::Property),
                            ));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Enum(e) => {
                let range = sv.name_range(e.name);
                out.push(ref_count_lens(range, e.name, uri, all_docs, None));
                for member in e.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) => {
                            let method_range = sv.name_range(m.name);
                            out.push(ref_count_lens(method_range, m.name, uri, all_docs, None));
                        }
                        EnumMemberKind::Case(c) => {
                            let case_range = sv.name_range(c.name);
                            out.push(ref_count_lens(case_range, c.name, uri, all_docs, None));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_lenses(inner, sv, uri, all_docs, out);
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
    uri: &Url,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    kind: Option<SymbolKind>,
) -> CodeLens {
    let locations = find_references(name, all_docs, false, kind);
    let count = locations.len();
    let label = match count {
        0 => "0 references".to_string(),
        1 => "1 reference".to_string(),
        n => format!("{n} references"),
    };
    CodeLens {
        range,
        command: Some(Command {
            title: label,
            command: "editor.action.showReferences".to_string(),
            arguments: Some(vec![json!(uri), json!(range.start), json!(locations)]),
        }),
        data: None,
    }
}

fn impl_count_lens(
    range: tower_lsp::lsp_types::Range,
    uri: &Url,
    locations: Vec<tower_lsp::lsp_types::Location>,
) -> CodeLens {
    let count = locations.len();
    let label = match count {
        0 => "0 implementations".to_string(),
        1 => "1 implementation".to_string(),
        n => format!("{n} implementations"),
    };
    CodeLens {
        range,
        command: Some(Command {
            title: label,
            command: "editor.action.showReferences".to_string(),
            arguments: Some(vec![json!(uri), json!(range.start), json!(locations)]),
        }),
        data: None,
    }
}

fn overrides_lens(
    range: tower_lsp::lsp_types::Range,
    uri: &Url,
    parent_class: &str,
    method_name: &str,
    parent_location: tower_lsp::lsp_types::Location,
) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command {
            title: format!("overrides {}::{}", parent_class, method_name),
            command: "editor.action.showReferences".to_string(),
            arguments: Some(vec![
                json!(uri),
                json!(range.start),
                json!(vec![parent_location]),
            ]),
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

/// Count how many classes across `all_docs` use `trait_name` via a `use` statement.
fn trait_usage_locations(
    trait_name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<tower_lsp::lsp_types::Location> {
    let mut out = Vec::new();
    for (uri, doc) in all_docs {
        let sv = doc.view();
        collect_trait_usages_in_stmts(trait_name, &doc.program().stmts, sv, uri, &mut out);
    }
    out
}

fn collect_trait_usages_in_stmts(
    trait_name: &str,
    stmts: &[php_ast::Stmt<'_, '_>],
    sv: SourceView<'_>,
    uri: &Url,
    out: &mut Vec<tower_lsp::lsp_types::Location>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let uses_trait = c.members.iter().any(|m| {
                    if let ClassMemberKind::TraitUse(t) = &m.kind {
                        t.traits
                            .iter()
                            .any(|name| name.to_string_repr().as_ref() == trait_name)
                    } else {
                        false
                    }
                });
                if uses_trait && let Some(class_name) = c.name {
                    out.push(tower_lsp::lsp_types::Location {
                        uri: uri.clone(),
                        range: sv.name_range(class_name),
                    });
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_trait_usages_in_stmts(trait_name, inner, sv, uri, out);
                }
            }
            _ => {}
        }
    }
}

/// Direct supertypes of `c` — the extended parent class (resolved to its
/// canonical short name) plus every trait listed in `use` clauses. Order is
/// stable: extends first, then traits in source order. Duplicates are removed.
fn collect_direct_supertypes(
    c: &php_ast::ClassDecl<'_, '_>,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(extends) = &c.extends {
        let parent_short = extends.to_string_repr().into_owned();
        let resolved = all_docs
            .iter()
            .find_map(|(_, doc)| parent_class_name(doc, &parent_short))
            .unwrap_or(parent_short);
        out.push(resolved);
    }
    for member in c.members.iter() {
        if let ClassMemberKind::TraitUse(t) = &member.kind {
            for name in t.traits.iter() {
                let s = name.to_string_repr().into_owned();
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// Find the declaration location of `method_name` on a class or trait named
/// `parent_name`, if any.
fn parent_method_location(
    parent_name: &str,
    method_name: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
) -> Option<tower_lsp::lsp_types::Location> {
    for (uri, doc) in all_docs {
        let sv = doc.view();
        if let Some(range) =
            find_method_name_range(&doc.program().stmts, parent_name, method_name, sv)
        {
            return Some(tower_lsp::lsp_types::Location {
                uri: uri.clone(),
                range,
            });
        }
    }
    None
}

fn find_method_name_range(
    stmts: &[php_ast::Stmt<'_, '_>],
    parent_name: &str,
    method_name: &str,
    sv: SourceView<'_>,
) -> Option<tower_lsp::lsp_types::Range> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) if c.name == Some(parent_name) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(sv.name_range(m.name));
                    }
                }
            }
            StmtKind::Trait(t) if t.name == parent_name => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(sv.name_range(m.name));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_method_name_range(inner, parent_name, method_name, sv)
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

/// A method is a test if its name starts with `test` (PHPUnit convention),
/// if its leading docblock contains `@test`, or if it carries a `#[Test]`
/// or `#[PHPUnit\Framework\Attributes\Test]` PHP attribute.
fn is_test_method(source: &str, m: &php_ast::MethodDecl<'_, '_>, member_start: u32) -> bool {
    if m.name.starts_with("test") {
        return true;
    }
    let has_test_attr = m.attributes.iter().any(|attr| {
        let span = attr.name.span();
        let attr_name = source
            .get(span.start as usize..span.end as usize)
            .unwrap_or("");
        attr_name == "Test" || attr_name.ends_with("\\Test")
    });
    if has_test_attr {
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
        assert_eq!(
            lenses.len(),
            1,
            "expected exactly 1 lens for a top-level function"
        );
        let cmd = lenses[0]
            .command
            .as_ref()
            .expect("lens should have a command");
        // No callers -> "0 references"
        assert_eq!(
            cmd.title, "0 references",
            "unused function should show '0 references'"
        );
        assert_eq!(
            cmd.command, "editor.action.showReferences",
            "command name should be 'editor.action.showReferences'"
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
        assert_eq!(
            cmd.command, "php-lsp.runTest",
            "command name must be 'php-lsp.runTest'"
        );
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
        assert_eq!(
            lenses.len(),
            1,
            "expected exactly 1 lens for a class declaration"
        );
        let cmd = lenses[0]
            .command
            .as_ref()
            .expect("lens should have a command");
        assert_eq!(
            cmd.title, "0 references",
            "unused class should show '0 references'"
        );
    }

    #[test]
    fn emits_lens_for_interface() {
        let src = "<?php\ninterface Countable {}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        // Interface gets a ref-count lens + an implementations lens.
        assert_eq!(
            lenses.len(),
            2,
            "expected 2 lenses (ref-count + implementations) for interface"
        );
        let titles: Vec<&str> = lenses
            .iter()
            .filter_map(|l| l.command.as_ref())
            .map(|c| c.title.as_str())
            .collect();
        assert!(
            titles
                .iter()
                .any(|t| t.ends_with("reference") || t.ends_with("references")),
            "one lens should be a reference count, got: {:?}",
            titles
        );
        assert!(
            titles.iter().any(|t| t.contains("implementation")),
            "one lens should be an implementations count, got: {:?}",
            titles
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
        assert!(
            impl_lens.is_some(),
            "expected implementations lens on abstract class"
        );
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
    fn test_attribute_triggers_run_test_lens() {
        let src = "<?php\nclass FooTest {\n#[Test]\npublic function it_does_something() {}\n}";
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
            "expected Run test lens from #[Test] attribute"
        );
    }

    #[test]
    fn fqn_test_attribute_triggers_run_test_lens() {
        let src = "<?php\nclass FooTest {\n#[PHPUnit\\Framework\\Attributes\\Test]\npublic function it_does_something() {}\n}";
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
            "expected Run test lens from fully-qualified #[PHPUnit\\Framework\\Attributes\\Test] attribute"
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
                .map_or(false, |c| c.command == "editor.action.showReferences")
        });
        let cmd = ref_lens
            .expect("expected a showReferences lens")
            .command
            .as_ref()
            .unwrap();
        assert_eq!(
            cmd.title, "0 references",
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
            cmd.command, "php-lsp.runTest",
            "command name must be exactly 'php-lsp.runTest'"
        );
        assert_eq!(
            cmd.title, "▶ Run test",
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

    #[test]
    fn emits_trait_usages_lens_with_zero_when_unused() {
        let src = "<?php\ntrait Loggable {}";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let impl_lens = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("implementation"))
        });
        assert!(
            impl_lens.is_some(),
            "expected a usages/implementations lens for trait"
        );
        assert!(
            impl_lens
                .unwrap()
                .command
                .as_ref()
                .unwrap()
                .title
                .starts_with("0"),
            "expected 0 implementations when no class uses the trait"
        );
    }

    #[test]
    fn emits_trait_usages_lens_counts_classes_using_trait() {
        let src = "<?php\ntrait Loggable {}\nclass A { use Loggable; }\nclass B { use Loggable; }";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        let impl_lens = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("implementation"))
        });
        assert!(
            impl_lens.is_some(),
            "expected a usages lens for trait with users"
        );
        let title = &impl_lens.unwrap().command.as_ref().unwrap().title;
        assert!(
            title.starts_with("2"),
            "expected 2 implementations for trait used by 2 classes, got: {}",
            title
        );
    }

    #[test]
    fn trait_usages_lens_counts_across_multiple_docs() {
        let trait_src = "<?php\ntrait Loggable {}";
        let user_a = "<?php\nclass A { use Loggable; }";
        let user_b = "<?php\nclass B { use Loggable; }";
        let d = doc(trait_src);
        let docs = vec![
            (uri("/trait.php"), Arc::new(doc(trait_src))),
            (uri("/a.php"), Arc::new(doc(user_a))),
            (uri("/b.php"), Arc::new(doc(user_b))),
        ];
        let lenses = code_lenses(&uri("/trait.php"), &d, &docs);
        let impl_lens = lenses.iter().find(|l| {
            l.command
                .as_ref()
                .map_or(false, |c| c.title.contains("implementation"))
        });
        assert!(
            impl_lens.is_some(),
            "expected a usages lens for trait used across multiple docs"
        );
        let title = &impl_lens.unwrap().command.as_ref().unwrap().title;
        assert!(
            title.starts_with("2"),
            "expected 2 implementations across docs, got: {}",
            title
        );
    }

    /// Invariant: every lens that emits `editor.action.showReferences` must
    /// pass `[uri, position, locations]` as arguments. Catches the bug class
    /// where a lens was wired with `arguments: None` and silently did nothing.
    #[test]
    fn show_references_lenses_always_have_three_arguments() {
        let src = "<?php
namespace App;
interface Animal { public function speak(): string; }
trait Barker { public function bark(): string { return 'woof'; } }
abstract class Base { public function greet(): string { return 'hi'; } }
class Dog extends Base implements Animal {
    use Barker;
    public string $breed = '';
    public function __construct(public int $age) {}
    public function speak(): string { return 'woof'; }
    public function greet(): string { return 'hello'; }
}
function topLevel(): void {}
";
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);

        let mut seen_any = false;
        for lens in &lenses {
            let Some(cmd) = &lens.command else { continue };
            if cmd.command == "editor.action.showReferences" {
                seen_any = true;
                let args = cmd.arguments.as_ref().unwrap_or_else(|| {
                    panic!(
                        "lens {:?} uses editor.action.showReferences but has no arguments",
                        cmd.title
                    )
                });
                assert_eq!(
                    args.len(),
                    3,
                    "lens {:?} must pass [uri, position, locations]; got {} args",
                    cmd.title,
                    args.len()
                );
                assert!(args[2].is_array(), "3rd arg (locations) must be an array");
            }
        }
        assert!(
            seen_any,
            "fixture should produce at least one editor.action.showReferences lens"
        );
    }

    #[test]
    fn emits_lens_for_class_property() {
        // Regular class property: `public string $name;` should get a ref-count lens.
        let src = r#"<?php
class User {
    public string $name = '';
    public function rename(string $new): void { $this->name = $new; }
    public function who(): string { return $this->name; }
}"#;
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        // The lens should sit on the property name range (line with `public string $name`).
        // Look for any lens whose title is a reference count and whose range overlaps the property line.
        let prop_lens = lenses.iter().find(|l| {
            let title_ok = l
                .command
                .as_ref()
                .map_or(false, |c| c.title.contains("reference"));
            // $name appears on line index 2 (0-based) in the fixture.
            title_ok && l.range.start.line == 2
        });
        assert!(
            prop_lens.is_some(),
            "expected a references lens on the property declaration line"
        );
        let cmd = prop_lens.unwrap().command.as_ref().unwrap();
        // Two accesses: `$this->name = $new` and `return $this->name`.
        assert!(
            cmd.title.starts_with("2"),
            "expected '2 references' for the property, got {:?}",
            cmd.title
        );
    }

    #[test]
    fn emits_lens_for_promoted_constructor_property() {
        // Constructor-promoted property: `public function __construct(public int $age)`.
        let src = r#"<?php
class Dog {
    public function __construct(public int $age) {}
    public function birthday(): void { $this->age++; }
    public function years(): int { return $this->age; }
}"#;
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        // Promoted param lens sits on the __construct line — look for a '2 references' lens there.
        let promoted_lens = lenses.iter().find(|l| {
            let cmd_ok = l
                .command
                .as_ref()
                .map_or(false, |c| c.title.contains("reference"));
            // __construct is on line 2 (0-based).
            cmd_ok && l.range.start.line == 2 && l.command.as_ref().unwrap().title.starts_with("2")
        });
        assert!(
            promoted_lens.is_some(),
            "expected a '2 references' lens on the promoted-property declaration line"
        );
    }

    #[test]
    fn property_lens_does_not_match_same_named_method() {
        // A method and a property with the same identifier must not cross-count.
        let src = r#"<?php
class Foo {
    public string $name = '';
    public function name(): string { return $this->name; }
}
$f = new Foo();
echo $f->name;
$f->name();
"#;
        let d = doc(src);
        let docs = vec![(uri("/a.php"), Arc::new(doc(src)))];
        let lenses = code_lenses(&uri("/a.php"), &d, &docs);
        // Property lens is on line 2 (the `$name` declaration line).
        let prop_title = lenses
            .iter()
            .find(|l| {
                l.range.start.line == 2
                    && l.command
                        .as_ref()
                        .map_or(false, |c| c.title.contains("reference"))
            })
            .and_then(|l| l.command.as_ref())
            .map(|c| c.title.clone())
            .expect("expected a property lens on the $name declaration line");
        // Property accesses: `$this->name` in the method body + `$f->name` below.
        // The property lens must NOT include the method call `$f->name()`.
        assert!(
            prop_title.starts_with("2"),
            "property lens should count only property accesses, not method calls; got {:?}",
            prop_title
        );
    }
}
