/// Code action: "Implement missing methods"
///
/// When a class `implements` an interface or `extends` an abstract class,
/// this action generates stub methods for any abstract/interface methods
/// that are not yet implemented in the class body.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind, Visibility};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView, format_type_hint};
use crate::hover::format_params_str;
use crate::text::fqn_short_name;

struct MethodStub {
    name: String,
    visibility: &'static str,
    is_static: bool,
    params: String,
    return_type: Option<String>,
}

pub fn implement_missing_actions(
    _source: &str,
    doc: &ParsedDoc,
    all_docs: &[(Uri, Arc<ParsedDoc>)],
    range: Range,
    uri: &Uri,
    file_imports: &HashMap<String, String>,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let mut actions = Vec::new();
    collect_actions(
        &doc.program().stmts,
        sv,
        all_docs,
        file_imports,
        range,
        uri,
        &mut actions,
    );
    actions
}

/// Short names (`implements`/`extends` targets) of every class in `stmts`
/// whose span overlaps `range` — the search needles for a text prefilter
/// before parsing candidate files to find their declaring documents. Uses
/// the same span-overlap check as `collect_actions`'s per-class walk.
pub(crate) fn target_type_names(
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    range: Range,
) -> Vec<String> {
    let mut names = Vec::new();
    collect_target_type_names(stmts, sv, range, &mut names);
    names
}

fn collect_target_type_names(
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    range: Range,
    out: &mut Vec<String>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let class_start = sv.position_of(stmt.span.start).line;
                let class_end = sv.position_of(stmt.span.end).line;
                if class_start > range.end.line || class_end < range.start.line {
                    continue;
                }
                for iface in c.implements.iter() {
                    out.push(fqn_short_name(&iface.to_string_repr()).to_string());
                }
                if let Some(parent) = &c.extends {
                    out.push(fqn_short_name(&parent.to_string_repr()).to_string());
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_target_type_names(&inner.stmts, sv, range, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_actions(
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    all_docs: &[(Uri, Arc<ParsedDoc>)],
    file_imports: &HashMap<String, String>,
    range: Range,
    uri: &Uri,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let class_start = sv.position_of(stmt.span.start).line;
                let class_end = sv.position_of(stmt.span.end).line;
                if class_start > range.end.line || class_end < range.start.line {
                    continue;
                }

                let existing: HashSet<String> = c
                    .body
                    .members
                    .iter()
                    .filter_map(|m| {
                        if let ClassMemberKind::Method(method) = &m.kind {
                            Some(method.name.to_string())
                        } else {
                            None
                        }
                    })
                    .collect();

                let mut missing: Vec<MethodStub> = Vec::new();

                for iface in c.implements.iter() {
                    let iface_name = iface.to_string_repr().into_owned();
                    let short = fqn_short_name(&iface_name).to_string();
                    // Try to resolve through `use` imports first; fall back to short-name scan.
                    let fqn = file_imports.get(&short).cloned();
                    for stub in abstract_methods_of(&short, fqn.as_deref(), all_docs) {
                        if !existing.contains(&stub.name) {
                            missing.push(stub);
                        }
                    }
                }

                if let Some(parent) = &c.extends {
                    let parent_name = parent.to_string_repr().into_owned();
                    let short = fqn_short_name(&parent_name).to_string();
                    let fqn = file_imports.get(&short).cloned();
                    for stub in abstract_methods_of(&short, fqn.as_deref(), all_docs) {
                        if !existing.contains(&stub.name) {
                            missing.push(stub);
                        }
                    }
                }

                // Deduplicate by method name (multiple interfaces may declare the same method).
                {
                    let mut seen = HashSet::new();
                    missing.retain(|s| seen.insert(s.name.clone()));
                }

                if missing.is_empty() {
                    continue;
                }

                let mut stub_text = generate_stub_text(&missing);
                let closing_pos = sv.position_of(stmt.span.end.saturating_sub(1));
                let insert_pos = closing_pos;
                // For single-line classes `class Foo {}` the `}` is not at column 0,
                // so we need a leading newline to avoid the stub running onto the
                // opening brace of the class.
                if closing_pos.character > 0 {
                    stub_text = format!("\n{stub_text}");
                }
                let edit = TextEdit {
                    range: Range {
                        start: insert_pos,
                        end: insert_pos,
                    },
                    new_text: stub_text,
                };
                let mut changes = HashMap::new();
                changes.insert(uri.clone(), vec![edit]);

                let n = missing.len();
                let title = if n == 1 {
                    "Implement missing method".to_string()
                } else {
                    format!("Implement {n} missing methods")
                };
                out.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title,
                    kind: Some(CodeActionKind::QUICKFIX),
                    // The only way to satisfy an interface's abstract methods
                    // is to implement them — there's no competing quickfix
                    // for this diagnostic, so editors can safely offer this
                    // as the auto-apply default (e.g. VS Code's Cmd+.).
                    is_preferred: Some(true),
                    edit: Some(WorkspaceEdit {
                        changes: Some(changes),
                        ..Default::default()
                    }),
                    ..Default::default()
                }));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_actions(&inner.stmts, sv, all_docs, file_imports, range, uri, out);
                }
            }
            _ => {}
        }
    }
}

/// Collect abstract/interface methods declared by `name` across all documents.
///
/// When `fqn` is provided (resolved from a `use` statement), the search uses
/// FQN-aware matching only — it looks for a document whose namespace + class
/// name matches the FQN exactly.  This avoids picking up a different class that
/// happens to share the same short name in another namespace.
///
/// When `fqn` is `None` (no `use` import found), falls back to a plain
/// short-name scan across all documents, preserving the original behaviour.
fn abstract_methods_of(
    name: &str,
    fqn: Option<&str>,
    all_docs: &[(Uri, Arc<ParsedDoc>)],
) -> Vec<MethodStub> {
    if let Some(fqn) = fqn {
        // FQN-aware pass: only return stubs when the exact namespace matches.
        // Do NOT fall back to short-name scan to avoid picking the wrong class.
        for (_, doc) in all_docs {
            if let Some(stubs) = collect_abstract_methods_fqn(&doc.program().stmts, fqn, "") {
                return stubs;
            }
        }
        return vec![];
    }

    // Short-name fallback (no `use` import): scan all docs as before.
    for (_, doc) in all_docs {
        if let Some(stubs) = collect_abstract_methods(&doc.program().stmts, name) {
            return stubs;
        }
    }
    vec![]
}

/// Like `collect_abstract_methods` but matches the fully-qualified name
/// `namespace\ClassName` by tracking the current namespace prefix while
/// recursing into `StmtKind::Namespace` blocks (both braced and unbraced).
fn collect_abstract_methods_fqn(
    stmts: &[Stmt<'_, '_>],
    fqn: &str,
    current_ns: &str,
) -> Option<Vec<MethodStub>> {
    // The expected short name is the last segment of the FQN.
    let short = fqn_short_name(fqn);
    // For unbraced namespaces (`namespace Foo;`) the active namespace changes
    // mid-statement-list; track it mutably as we iterate.
    let mut active_ns = current_ns.to_string();

    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Interface(i) if i.name == short => {
                // Verify the namespace matches.
                let declared_fqn = if active_ns.is_empty() {
                    i.name.to_string()
                } else {
                    format!("{}\\{}", active_ns, i.name)
                };
                if fqn_eq(fqn, &declared_fqn) {
                    let stubs = i
                        .body
                        .members
                        .iter()
                        .filter_map(|m| {
                            if let ClassMemberKind::Method(method) = &m.kind {
                                Some(MethodStub {
                                    name: method.name.to_string(),
                                    visibility: "public",
                                    is_static: method.is_static,
                                    params: format_params_str(&method.params),
                                    return_type: method
                                        .return_type
                                        .as_ref()
                                        .map(|t| format_type_hint(t)),
                                })
                            } else {
                                None
                            }
                        })
                        .collect();
                    return Some(stubs);
                }
            }
            StmtKind::Class(c)
                if c.name.as_ref().map(|n| n.to_string()) == Some(short.to_string())
                    && c.modifiers.is_abstract =>
            {
                let declared_fqn = if active_ns.is_empty() {
                    short.to_string()
                } else {
                    format!("{}\\{}", active_ns, short)
                };
                if fqn_eq(fqn, &declared_fqn) {
                    let stubs = c
                        .body
                        .members
                        .iter()
                        .filter_map(|m| {
                            if let ClassMemberKind::Method(method) = &m.kind {
                                if method.is_abstract {
                                    Some(MethodStub {
                                        name: method.name.to_string(),
                                        visibility: visibility_str(method.visibility.as_ref()),
                                        is_static: method.is_static,
                                        params: format_params_str(&method.params),
                                        return_type: method
                                            .return_type
                                            .as_ref()
                                            .map(|t| format_type_hint(t)),
                                    })
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .collect();
                    return Some(stubs);
                }
            }
            StmtKind::Namespace(ns) => {
                let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().into_owned());
                match &ns.body {
                    NamespaceBody::Braced(inner) => {
                        let child_ns = match &ns_name {
                            Some(n) if !active_ns.is_empty() => {
                                format!("{}\\{}", active_ns, n)
                            }
                            Some(n) => n.clone(),
                            None => active_ns.clone(),
                        };
                        if let Some(stubs) =
                            collect_abstract_methods_fqn(&inner.stmts, fqn, &child_ns)
                        {
                            return Some(stubs);
                        }
                    }
                    NamespaceBody::Simple => {
                        // Unbraced form: all subsequent statements are in this namespace.
                        active_ns = ns_name.unwrap_or_default();
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Compare two FQNs ignoring a leading backslash.
fn fqn_eq(a: &str, b: &str) -> bool {
    a.trim_start_matches('\\') == b.trim_start_matches('\\')
}

fn collect_abstract_methods(stmts: &[Stmt<'_, '_>], name: &str) -> Option<Vec<MethodStub>> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Interface(i) if i.name == name => {
                let stubs = i
                    .body
                    .members
                    .iter()
                    .filter_map(|m| {
                        if let ClassMemberKind::Method(method) = &m.kind {
                            Some(MethodStub {
                                name: method.name.to_string(),
                                visibility: "public",
                                is_static: method.is_static,
                                params: format_params_str(&method.params),
                                return_type: method
                                    .return_type
                                    .as_ref()
                                    .map(|t| format_type_hint(t)),
                            })
                        } else {
                            None
                        }
                    })
                    .collect();
                return Some(stubs);
            }
            StmtKind::Class(c)
                if c.name.as_ref().map(|n| n.to_string()) == Some(name.to_string())
                    && c.modifiers.is_abstract =>
            {
                let stubs = c
                    .body
                    .members
                    .iter()
                    .filter_map(|m| {
                        if let ClassMemberKind::Method(method) = &m.kind {
                            if method.is_abstract {
                                Some(MethodStub {
                                    name: method.name.to_string(),
                                    visibility: visibility_str(method.visibility.as_ref()),
                                    is_static: method.is_static,
                                    params: format_params_str(&method.params),
                                    return_type: method
                                        .return_type
                                        .as_ref()
                                        .map(|t| format_type_hint(t)),
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();
                return Some(stubs);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(stubs) = collect_abstract_methods(&inner.stmts, name)
                {
                    return Some(stubs);
                }
            }
            _ => {}
        }
    }
    None
}

fn visibility_str(v: Option<&Visibility>) -> &'static str {
    match v {
        Some(Visibility::Protected) => "protected",
        Some(Visibility::Private) => "private",
        _ => "public",
    }
}

fn generate_stub_text(stubs: &[MethodStub]) -> String {
    let mut text = String::new();
    for stub in stubs {
        let static_kw = if stub.is_static { "static " } else { "" };
        let ret = match &stub.return_type {
            Some(t) => format!(": {t}"),
            None => String::new(),
        };
        text.push_str(&format!(
            "    {} {}function {}({}){ret}\n    {{\n        throw new \\RuntimeException('Not implemented');\n    }}\n\n",
            stub.visibility, static_kw, stub.name, stub.params
        ));
    }
    text
}
