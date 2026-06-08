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
                let name = f.name.as_str().unwrap_or_default();
                let range = sv.name_range(name);
                out.push(ref_count_lens(range, name, uri, all_docs, None));
            }
            StmtKind::Class(c) => {
                if let Some(class_name) = c.name {
                    let class_name_str = class_name.as_str().unwrap_or_default();
                    let class_range = sv.name_range(class_name_str);
                    out.push(ref_count_lens(
                        class_range,
                        class_name_str,
                        uri,
                        all_docs,
                        None,
                    ));

                    // Implementations count for abstract classes (classes extending this).
                    if c.modifiers.is_abstract {
                        let impls = find_implementations(class_name_str, None, all_docs);
                        out.push(impl_count_lens(class_range, uri, impls));
                    }

                    // Direct supertypes — extends parent + used traits — checked once
                    // per class for overrides lookups on each method.
                    let parents = collect_direct_supertypes(c, all_docs);

                    for member in c.body.members.iter() {
                        match &member.kind {
                            ClassMemberKind::Method(m) => {
                                let method_name = m.name.as_str().unwrap_or_default();
                                let method_range = sv.name_range(method_name);
                                out.push(ref_count_lens(
                                    method_range,
                                    method_name,
                                    uri,
                                    all_docs,
                                    None,
                                ));

                                if is_test_method(sv.source(), m) {
                                    out.push(run_test_lens(
                                        method_range,
                                        uri,
                                        class_name_str,
                                        method_name,
                                    ));
                                }

                                // Overrides lens: emit for each direct supertype (parent class
                                // or used trait) that declares a method with the same name.
                                for parent_name in &parents {
                                    if let Some(parent_loc) =
                                        parent_method_location(parent_name, method_name, all_docs)
                                    {
                                        out.push(overrides_lens(
                                            method_range,
                                            uri,
                                            parent_name,
                                            method_name,
                                            parent_loc,
                                        ));
                                    }
                                }

                                // Constructor-promoted params: `public function __construct(public string $name)`.
                                if m.name == "__construct" {
                                    for p in m.params.iter() {
                                        if p.visibility.is_some() {
                                            let param_name = p.name.as_str().unwrap_or_default();
                                            let prop_range = sv.name_range(param_name);
                                            out.push(ref_count_lens(
                                                prop_range,
                                                param_name,
                                                uri,
                                                all_docs,
                                                Some(SymbolKind::Property),
                                            ));
                                        }
                                    }
                                }
                            }
                            ClassMemberKind::Property(p) => {
                                let prop_name = p.name.as_str().unwrap_or_default();
                                let prop_range = sv.name_range(prop_name);
                                out.push(ref_count_lens(
                                    prop_range,
                                    prop_name,
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
                let name = i.name.as_str().unwrap_or_default();
                let range = sv.name_range(name);
                out.push(ref_count_lens(range, name, uri, all_docs, None));
                // Implementations count lens.
                let impls = find_implementations(name, None, all_docs);
                out.push(impl_count_lens(range, uri, impls));
            }
            StmtKind::Trait(t) => {
                let trait_name = t.name.as_str().unwrap_or_default();
                let range = sv.name_range(trait_name);
                out.push(ref_count_lens(range, trait_name, uri, all_docs, None));
                // Usages: classes that `use` this trait.
                let usages = trait_usage_locations(trait_name, all_docs);
                out.push(impl_count_lens(range, uri, usages));
                for member in t.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let method_name = m.name.as_str().unwrap_or_default();
                            let method_range = sv.name_range(method_name);
                            out.push(ref_count_lens(
                                method_range,
                                method_name,
                                uri,
                                all_docs,
                                None,
                            ));
                        }
                        ClassMemberKind::Property(p) => {
                            let prop_name = p.name.as_str().unwrap_or_default();
                            let prop_range = sv.name_range(prop_name);
                            out.push(ref_count_lens(
                                prop_range,
                                prop_name,
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
                let enum_name = e.name.as_str().unwrap_or_default();
                let range = sv.name_range(enum_name);
                out.push(ref_count_lens(range, enum_name, uri, all_docs, None));
                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) => {
                            let method_name = m.name.as_str().unwrap_or_default();
                            let method_range = sv.name_range(method_name);
                            out.push(ref_count_lens(
                                method_range,
                                method_name,
                                uri,
                                all_docs,
                                None,
                            ));
                        }
                        EnumMemberKind::Case(c) => {
                            let case_name = c.name.as_str().unwrap_or_default();
                            let case_range = sv.name_range(case_name);
                            out.push(ref_count_lens(case_range, case_name, uri, all_docs, None));
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_lenses(&inner.stmts, sv, uri, all_docs, out);
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
                let uses_trait = c.body.members.iter().any(|m| {
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
                        range: sv.name_range_in_span(class_name.or_error(), stmt.span),
                    });
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_trait_usages_in_stmts(trait_name, &inner.stmts, sv, uri, out);
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
    for member in c.body.members.iter() {
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
            StmtKind::Class(c) if c.name.as_ref().and_then(|n| n.as_str()) == Some(parent_name) => {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(sv.name_range(m.name.as_str().unwrap_or_default()));
                    }
                }
            }
            StmtKind::Trait(t) if t.name == parent_name => {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == method_name
                    {
                        return Some(sv.name_range(m.name.as_str().unwrap_or_default()));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) =
                        find_method_name_range(&inner.stmts, parent_name, method_name, sv)
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
fn is_test_method(source: &str, m: &php_ast::MethodDecl<'_, '_>) -> bool {
    if m.name
        .as_str()
        .map(|s| s.starts_with("test"))
        .unwrap_or(false)
    {
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
    m.doc_comment
        .as_ref()
        .is_some_and(|c| c.text.contains("@test"))
}
