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
///
/// Counts and locations come from mir's inverted indexes (reference posting
/// lists and subtype edges) plus the salsa workspace index — never from a
/// per-request AST walk over the workspace.
use std::collections::HashMap;

use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Stmt, StmtKind};
use serde_json::json;
use tower_lsp_server::ls_types::{CodeLens, Command, Location, Uri};

use crate::document::ast::{ParsedDoc, SourceView};
use crate::document::document_store::DocumentStore;
use crate::navigation::moniker::resolve_fqn;
use crate::navigation::references::{dedup_ref_locations, session_tuple_to_location};
use crate::text::fqn_short_name;

/// Build all code lenses for `uri`/`doc`, answering counts from the store's
/// mir-backed indexes.
///
/// `cancel_rev` is threaded into mir's index queries so a concurrent edit
/// aborts them at phase boundaries. `should_cancel` is polled between
/// top-level declarations; when it returns `true` the function returns an
/// empty `Vec` immediately. Pass `|| false` for requests that do not need
/// cooperative cancellation.
pub fn code_lenses(
    uri: &Uri,
    doc: &ParsedDoc,
    store: &DocumentStore,
    imports: &HashMap<String, String>,
    cancel_rev: Option<u64>,
    should_cancel: impl Fn() -> bool,
) -> Vec<CodeLens> {
    let env = LensEnv {
        uri,
        doc,
        store,
        imports,
        cancel_rev,
    };
    let sv = doc.view();
    let mut slots = Vec::new();
    collect_lenses(
        &doc.program().stmts,
        sv,
        "",
        &env,
        &mut slots,
        &should_cancel,
    );
    if should_cancel() {
        return Vec::new();
    }

    // Every `RefCount` slot's candidate scope is resolved together in one
    // workspace pass (see `DocumentStore::batch_reference_candidate_files`)
    // instead of one pass per declaration — a class with many methods/
    // properties would otherwise re-scan the whole workspace once per
    // member just to compute this request's reference counts.
    let symbols: Vec<mir_analyzer::Name> = slots
        .iter()
        .filter_map(|slot| match slot {
            LensSlot::RefCount { symbol, .. } => Some(symbol.clone()),
            LensSlot::Ready(_) => None,
        })
        .collect();
    let mut scopes = env
        .store
        .batch_reference_candidate_files(&symbols)
        .into_iter();

    let mut lenses = Vec::with_capacity(slots.len());
    for slot in slots {
        if should_cancel() {
            return Vec::new();
        }
        lenses.push(match slot {
            LensSlot::Ready(lens) => lens,
            LensSlot::RefCount { range, symbol } => {
                let files = scopes.next().expect("one scope per RefCount slot");
                env.ref_count_lens(range, symbol, &files)
            }
        });
    }
    lenses
}

/// Shared lookup context for one code-lens request.
struct LensEnv<'a> {
    uri: &'a Uri,
    doc: &'a ParsedDoc,
    store: &'a DocumentStore,
    imports: &'a HashMap<String, String>,
    cancel_rev: Option<u64>,
}

/// A lens that still needs its reference count resolved, or one that's
/// already fully built. Deferring `RefCount` lets [`code_lenses`] batch
/// every declaration's scope narrowing into one workspace pass after the
/// AST walk finishes, instead of narrowing per declaration during the walk.
enum LensSlot {
    Ready(CodeLens),
    RefCount {
        range: tower_lsp_server::ls_types::Range,
        symbol: mir_analyzer::Name,
    },
}

impl LensEnv<'_> {
    fn ref_count_lens(
        &self,
        range: tower_lsp_server::ls_types::Range,
        symbol: mir_analyzer::Name,
        files: &[std::sync::Arc<str>],
    ) -> CodeLens {
        let mut locations: Vec<Location> = self
            .store
            .indexed_references(&symbol, files, false, self.cancel_rev)
            .into_iter()
            .filter_map(session_tuple_to_location)
            .collect();
        dedup_ref_locations(&mut locations);
        let label = match locations.len() {
            0 => "0 references".to_string(),
            1 => "1 reference".to_string(),
            n => format!("{n} references"),
        };
        lens(range, self.uri, label, locations)
    }

    fn impl_count_lens(
        &self,
        range: tower_lsp_server::ls_types::Range,
        fqn: &str,
        include_trait_users: bool,
    ) -> CodeLens {
        let locations: Vec<Location> = self
            .store
            .indexed_subtype_classes(fqn, include_trait_users)
            .into_iter()
            .filter_map(|site| subtype_site_to_location(&site.file, &site.range))
            .collect();
        let label = match locations.len() {
            0 => "0 implementations".to_string(),
            1 => "1 implementation".to_string(),
            n => format!("{n} implementations"),
        };
        lens(range, self.uri, label, locations)
    }

    /// Declaration location of `method` on the class/trait `parent_fqn`,
    /// from the salsa workspace index. Prefers the exact-FQN class entry;
    /// falls back to any same-short-name class declaring the method.
    /// Walks the full `extends` ancestor chain starting at `parent_fqn` (not
    /// just the direct parent), so a method inherited unchanged from a
    /// grandparent (or further) still gets an "overrides" lens — a class two
    /// or more levels below the declaring ancestor previously got none at all.
    /// Returns the FQN of the ancestor that actually declares the method
    /// (which may differ from `parent_fqn` itself) alongside its location.
    fn parent_method_location(&self, parent_fqn: &str, method: &str) -> Option<(String, Location)> {
        let ws = self.store.get_workspace_index_salsa();
        let mut current = parent_fqn.trim_start_matches('\\').to_string();
        let mut seen = std::collections::HashSet::new();
        loop {
            if !seen.insert(current.clone()) {
                return None;
            }
            let chosen = self
                .store
                .resolve_class_ref_by_fqn_or_short_name_fallback(&ws, &current)?;
            let (uri, cls) = ws.at(chosen)?;
            let declaring_fqn = cls.fqn.trim_start_matches('\\').to_string();
            if let Some(m) = cls.methods.iter().find(|m| m.name.as_ref() == method) {
                let start = tower_lsp_server::ls_types::Position {
                    line: m.start_line,
                    character: m.name_char,
                };
                let end = tower_lsp_server::ls_types::Position {
                    line: m.start_line,
                    character: m.name_char + m.name.encode_utf16().count() as u32,
                };
                return Some((
                    declaring_fqn,
                    Location {
                        uri: uri.clone(),
                        range: tower_lsp_server::ls_types::Range { start, end },
                    },
                ));
            }
            let parent = cls.parent.as_deref()?;
            current = parent.trim_start_matches('\\').to_string();
        }
    }
}

fn collect_lenses(
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    enclosing_ns: &str,
    env: &LensEnv<'_>,
    out: &mut Vec<LensSlot>,
    should_cancel: &impl Fn() -> bool,
) {
    // `namespace App;` (unbraced) applies to every following statement.
    let mut ns: String = enclosing_ns.to_string();
    for stmt in stmts {
        if should_cancel() {
            out.clear();
            return;
        }
        match &stmt.kind {
            StmtKind::Function(f) => {
                let name = f.name.as_str().unwrap_or_default();
                let range = sv.name_range(name);
                out.push(LensSlot::RefCount {
                    range,
                    symbol: mir_analyzer::Name::function(fqn_in(&ns, name)),
                });
            }
            StmtKind::Class(c) => {
                if let Some(class_name) = c.name {
                    let class_name_str = class_name.as_str().unwrap_or_default();
                    let class_fqn = fqn_in(&ns, class_name_str);
                    let class_range = sv.name_range(class_name_str);
                    out.push(LensSlot::RefCount {
                        range: class_range,
                        symbol: mir_analyzer::Name::class(class_fqn.clone()),
                    });

                    // Implementations count for abstract classes (classes extending this).
                    if c.modifiers.is_abstract {
                        out.push(LensSlot::Ready(env.impl_count_lens(
                            class_range,
                            &class_fqn,
                            false,
                        )));
                    }

                    // Direct supertypes — extends parent + used traits — checked once
                    // per class for overrides lookups on each method.
                    let parents = collect_direct_supertypes(c, env.doc, env.imports);

                    for member in c.body.members.iter() {
                        match &member.kind {
                            ClassMemberKind::Method(m) => {
                                let method_name = m.name.as_str().unwrap_or_default();
                                let method_range = sv.name_range(method_name);
                                out.push(LensSlot::RefCount {
                                    range: method_range,
                                    symbol: mir_analyzer::Name::method(
                                        class_fqn.as_str(),
                                        method_name,
                                    ),
                                });

                                if is_test_method(sv.source(), m) {
                                    out.push(LensSlot::Ready(run_test_lens(
                                        method_range,
                                        env.uri,
                                        class_name_str,
                                        method_name,
                                    )));
                                }

                                // Overrides lens: emit for each direct supertype (parent class
                                // or used trait) that declares a method with the same name.
                                for parent_fqn in &parents {
                                    if let Some((declaring_fqn, parent_loc)) =
                                        env.parent_method_location(parent_fqn, method_name)
                                    {
                                        out.push(LensSlot::Ready(overrides_lens(
                                            method_range,
                                            env.uri,
                                            fqn_short_name(&declaring_fqn),
                                            method_name,
                                            parent_loc,
                                        )));
                                    }
                                }

                                // Constructor-promoted params: `public function __construct(public string $name)`.
                                if m.name == "__construct" {
                                    for p in m.params.iter() {
                                        if p.visibility.is_some() {
                                            let param_name = p.name.as_str().unwrap_or_default();
                                            let prop_range = sv.name_range(param_name);
                                            out.push(LensSlot::RefCount {
                                                range: prop_range,
                                                symbol: property_name(&class_fqn, param_name),
                                            });
                                        }
                                    }
                                }
                            }
                            ClassMemberKind::Property(p) => {
                                let prop_name = p.name.as_str().unwrap_or_default();
                                let prop_range = sv.name_range(prop_name);
                                out.push(LensSlot::RefCount {
                                    range: prop_range,
                                    symbol: property_name(&class_fqn, prop_name),
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }
            StmtKind::Interface(i) => {
                let name = i.name.as_str().unwrap_or_default();
                let fqn = fqn_in(&ns, name);
                let range = sv.name_range(name);
                out.push(LensSlot::RefCount {
                    range,
                    symbol: mir_analyzer::Name::class(fqn.clone()),
                });
                // Implementations count lens.
                out.push(LensSlot::Ready(env.impl_count_lens(range, &fqn, false)));
            }
            StmtKind::Trait(t) => {
                let trait_name = t.name.as_str().unwrap_or_default();
                let trait_fqn = fqn_in(&ns, trait_name);
                let range = sv.name_range(trait_name);
                out.push(LensSlot::RefCount {
                    range,
                    symbol: mir_analyzer::Name::class(trait_fqn.clone()),
                });
                // Usages: classes that `use` this trait (trait edges included).
                out.push(LensSlot::Ready(
                    env.impl_count_lens(range, &trait_fqn, true),
                ));
                for member in t.body.members.iter() {
                    match &member.kind {
                        ClassMemberKind::Method(m) => {
                            let method_name = m.name.as_str().unwrap_or_default();
                            let method_range = sv.name_range(method_name);
                            out.push(LensSlot::RefCount {
                                range: method_range,
                                symbol: mir_analyzer::Name::method(trait_fqn.as_str(), method_name),
                            });
                        }
                        ClassMemberKind::Property(p) => {
                            let prop_name = p.name.as_str().unwrap_or_default();
                            let prop_range = sv.name_range(prop_name);
                            out.push(LensSlot::RefCount {
                                range: prop_range,
                                symbol: property_name(&trait_fqn, prop_name),
                            });
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Enum(e) => {
                let enum_name = e.name.as_str().unwrap_or_default();
                let enum_fqn = fqn_in(&ns, enum_name);
                let range = sv.name_range(enum_name);
                out.push(LensSlot::RefCount {
                    range,
                    symbol: mir_analyzer::Name::class(enum_fqn.clone()),
                });
                for member in e.body.members.iter() {
                    match &member.kind {
                        EnumMemberKind::Method(m) => {
                            let method_name = m.name.as_str().unwrap_or_default();
                            let method_range = sv.name_range(method_name);
                            out.push(LensSlot::RefCount {
                                range: method_range,
                                symbol: mir_analyzer::Name::method(enum_fqn.as_str(), method_name),
                            });
                        }
                        EnumMemberKind::Case(c) => {
                            let case_name = c.name.as_str().unwrap_or_default();
                            let case_range = sv.name_range(case_name);
                            out.push(LensSlot::RefCount {
                                range: case_range,
                                symbol: mir_analyzer::Name::class_constant(
                                    enum_fqn.as_str(),
                                    case_name,
                                ),
                            });
                        }
                        _ => {}
                    }
                }
            }
            StmtKind::Namespace(nsd) => {
                let ns_name = nsd
                    .name
                    .as_ref()
                    .map(|n| n.to_string_repr().into_owned())
                    .unwrap_or_default();
                match &nsd.body {
                    NamespaceBody::Braced(inner) => {
                        collect_lenses(&inner.stmts, sv, &ns_name, env, out, should_cancel);
                    }
                    NamespaceBody::Simple => {
                        ns = ns_name;
                    }
                }
            }
            _ => {}
        }
    }
}

fn fqn_in(ns: &str, name: &str) -> String {
    if ns.is_empty() {
        name.to_string()
    } else {
        format!("{ns}\\{name}")
    }
}

fn property_name(class_fqn: &str, prop: &str) -> mir_analyzer::Name {
    mir_analyzer::Name::property(class_fqn, prop)
}

fn subtype_site_to_location(file: &str, range: &mir_analyzer::Range) -> Option<Location> {
    let uri = (file).parse::<Uri>().ok()?;
    // mir uses 1-based lines; 0-based columns.
    let line = range.start.line.saturating_sub(1);
    Some(Location {
        uri,
        range: tower_lsp_server::ls_types::Range {
            start: tower_lsp_server::ls_types::Position {
                line,
                character: range.start.column,
            },
            end: tower_lsp_server::ls_types::Position {
                line,
                character: range.end.column,
            },
        },
    })
}

fn lens(
    range: tower_lsp_server::ls_types::Range,
    uri: &Uri,
    title: String,
    locations: Vec<Location>,
) -> CodeLens {
    CodeLens {
        range,
        command: Some(Command {
            title,
            command: "editor.action.showReferences".to_string(),
            arguments: Some(vec![json!(uri), json!(range.start), json!(locations)]),
        }),
        data: None,
    }
}

fn overrides_lens(
    range: tower_lsp_server::ls_types::Range,
    uri: &Uri,
    parent_class: &str,
    method_name: &str,
    parent_location: Location,
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
    range: tower_lsp_server::ls_types::Range,
    uri: &Uri,
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

/// Direct supertypes of `c` as FQNs — the extended parent class plus every
/// trait listed in `use` clauses, resolved through this file's imports and
/// namespace. Order is stable: extends first, then traits in source order.
/// Duplicates are removed.
fn collect_direct_supertypes(
    c: &php_ast::ClassDecl<'_, '_>,
    doc: &ParsedDoc,
    imports: &HashMap<String, String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut resolve = |name: std::borrow::Cow<'_, str>| {
        let fqn = resolve_fqn(doc, &name, imports)
            .trim_start_matches('\\')
            .to_string();
        if !out.contains(&fqn) {
            out.push(fqn);
        }
    };
    if let Some(extends) = &c.extends {
        resolve(extends.to_string_repr());
    }
    for member in c.body.members.iter() {
        if let ClassMemberKind::TraitUse(t) = &member.kind {
            for name in t.traits.iter() {
                resolve(name.to_string_repr());
            }
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `should_cancel = || true` must return an empty Vec immediately without
    /// panicking, regardless of the document contents.
    #[test]
    fn code_lenses_cancelled_returns_empty() {
        use crate::document::ast::ParsedDoc;

        let source = "<?php\nclass Foo { public function bar(): void {} }\n";
        let doc = std::sync::Arc::new(ParsedDoc::parse(source));
        let uri = "file:///test.php".parse::<Uri>().unwrap();
        let store = DocumentStore::new();

        let lenses = code_lenses(&uri, &doc, &store, &HashMap::new(), None, || true);
        assert!(lenses.is_empty(), "cancelled sweep must return empty");
    }
}
