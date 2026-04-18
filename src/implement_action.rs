/// Code action: "Implement missing methods"
///
/// When a class `implements` an interface or `extends` an abstract class,
/// this action generates stub methods for any abstract/interface methods
/// that are not yet implemented in the class body.
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind, Visibility};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ast::{ParsedDoc, SourceView, format_type_hint};
use crate::hover::format_params_str;

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
    all_docs: &[(Url, Arc<ParsedDoc>)],
    range: Range,
    uri: &Url,
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

fn collect_actions(
    stmts: &[Stmt<'_, '_>],
    sv: SourceView<'_>,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    file_imports: &HashMap<String, String>,
    range: Range,
    uri: &Url,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                // Only consider classes whose declaration overlaps the requested range.
                let class_start = sv.position_of(stmt.span.start).line;
                let class_end = sv.position_of(stmt.span.end).line;
                if class_start > range.end.line || class_end < range.start.line {
                    continue;
                }

                // Gather method names already in this class.
                let existing: HashSet<String> = c
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

                // Interfaces this class implements.
                for iface in c.implements.iter() {
                    let iface_name = iface.to_string_repr().into_owned();
                    let short = last_segment(&iface_name).to_string();
                    // Try to resolve through `use` imports first; fall back to short-name scan.
                    let fqn = file_imports.get(&short).cloned();
                    for stub in abstract_methods_of(&short, fqn.as_deref(), all_docs) {
                        if !existing.contains(&stub.name) {
                            missing.push(stub);
                        }
                    }
                }

                // Abstract parent class (if any).
                if let Some(parent) = &c.extends {
                    let parent_name = parent.to_string_repr().into_owned();
                    let short = last_segment(&parent_name).to_string();
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

                let stub_text = generate_stub_text(&missing);
                // Insert just before the closing `}` of the class.
                let closing_line = sv.position_of(stmt.span.end.saturating_sub(1)).line;
                let insert_pos = Position {
                    line: closing_line,
                    character: 0,
                };
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
                    edit: Some(WorkspaceEdit {
                        changes: Some(changes),
                        ..Default::default()
                    }),
                    ..Default::default()
                }));
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_actions(inner, sv, all_docs, file_imports, range, uri, out);
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
    all_docs: &[(Url, Arc<ParsedDoc>)],
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
/// recursing into `StmtKind::Namespace` blocks.
fn collect_abstract_methods_fqn(
    stmts: &[Stmt<'_, '_>],
    fqn: &str,
    current_ns: &str,
) -> Option<Vec<MethodStub>> {
    // The expected short name is the last segment of the FQN.
    let short = last_segment(fqn);

    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Interface(i) if i.name == short => {
                // Verify the namespace matches.
                let declared_fqn = if current_ns.is_empty() {
                    i.name.to_string()
                } else {
                    format!("{}\\{}", current_ns, i.name)
                };
                if fqn_eq(fqn, &declared_fqn) {
                    let stubs = i
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
            StmtKind::Class(c) if c.name == Some(short) && c.modifiers.is_abstract => {
                let declared_fqn = if current_ns.is_empty() {
                    short.to_string()
                } else {
                    format!("{}\\{}", current_ns, short)
                };
                if fqn_eq(fqn, &declared_fqn) {
                    let stubs = c
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
                if let NamespaceBody::Braced(inner) = &ns.body {
                    let ns_name = ns.name.as_ref().map(|n| n.to_string_repr().into_owned());
                    let child_ns = match &ns_name {
                        Some(n) if !current_ns.is_empty() => format!("{}\\{}", current_ns, n),
                        Some(n) => n.clone(),
                        None => current_ns.to_string(),
                    };
                    if let Some(stubs) = collect_abstract_methods_fqn(inner, fqn, &child_ns) {
                        return Some(stubs);
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
            StmtKind::Class(c) if c.name == Some(name) && c.modifiers.is_abstract => {
                let stubs = c
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
                    && let Some(stubs) = collect_abstract_methods(inner, name)
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

fn last_segment(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(path: &str) -> Url {
        Url::parse(&format!("file://{path}")).unwrap()
    }

    fn doc(src: &str) -> (Url, Arc<ParsedDoc>) {
        (uri("/a.php"), Arc::new(ParsedDoc::parse(src.to_string())))
    }

    fn full_range() -> Range {
        Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: u32::MAX,
                character: u32::MAX,
            },
        }
    }

    #[test]
    fn generates_action_for_unimplemented_interface_method() {
        let iface_src = "<?php\ninterface Countable {\n    public function count(): int;\n}";
        let class_src = "<?php\nclass MyList implements Countable {\n}";
        let all_docs = vec![doc(iface_src), doc(class_src)];
        let class_doc = ParsedDoc::parse(class_src.to_string());
        let actions = implement_missing_actions(
            class_src,
            &class_doc,
            &all_docs,
            full_range(),
            &uri("/b.php"),
            &HashMap::new(),
        );
        assert!(!actions.is_empty(), "expected at least one action");
        if let CodeActionOrCommand::CodeAction(action) = &actions[0] {
            assert!(
                action.title.contains("missing method"),
                "title should mention 'missing method'"
            );
            let changes = action.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(
                edits[0].new_text.contains("function count()"),
                "stub should contain 'function count()'"
            );
            assert!(
                edits[0].new_text.contains(": int"),
                "stub should contain ': int' return type"
            );
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn no_action_when_all_methods_implemented() {
        let iface_src = "<?php\ninterface Countable {\n    public function count(): int;\n}";
        let class_src = "<?php\nclass MyList implements Countable {\n    public function count(): int { return 0; }\n}";
        let all_docs = vec![doc(iface_src), doc(class_src)];
        let class_doc = ParsedDoc::parse(class_src.to_string());
        let actions = implement_missing_actions(
            class_src,
            &class_doc,
            &all_docs,
            full_range(),
            &uri("/b.php"),
            &HashMap::new(),
        );
        assert!(
            actions.is_empty(),
            "no action needed when all methods are implemented"
        );
    }

    #[test]
    fn generates_action_for_abstract_class_method() {
        let abstract_src =
            "<?php\nabstract class Shape {\n    abstract public function area(): float;\n}";
        let class_src = "<?php\nclass Circle extends Shape {\n}";
        let all_docs = vec![doc(abstract_src), doc(class_src)];
        let class_doc = ParsedDoc::parse(class_src.to_string());
        let actions = implement_missing_actions(
            class_src,
            &class_doc,
            &all_docs,
            full_range(),
            &uri("/b.php"),
            &HashMap::new(),
        );
        assert!(!actions.is_empty(), "expected action for abstract method");
        if let CodeActionOrCommand::CodeAction(action) = &actions[0] {
            let changes = action.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(
                edits[0].new_text.contains("function area()"),
                "stub should contain 'function area()'"
            );
        }
    }

    #[test]
    fn stub_body_throws_runtime_exception() {
        let iface_src = "<?php\ninterface Runnable {\n    public function run(): void;\n}";
        let class_src = "<?php\nclass Task implements Runnable {\n}";
        let all_docs = vec![doc(iface_src), doc(class_src)];
        let class_doc = ParsedDoc::parse(class_src.to_string());
        let actions = implement_missing_actions(
            class_src,
            &class_doc,
            &all_docs,
            full_range(),
            &uri("/b.php"),
            &HashMap::new(),
        );
        assert!(!actions.is_empty());
        if let CodeActionOrCommand::CodeAction(action) = &actions[0] {
            let changes = action.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(
                edits[0]
                    .new_text
                    .contains("throw new \\RuntimeException('Not implemented')"),
                "stub body should throw RuntimeException, got: {}",
                edits[0].new_text
            );
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn resolves_interface_through_use_import() {
        // The interface is declared in a braced namespace; the class file imports it via `use`.
        let iface_src = "<?php\nnamespace App\\Contracts {\ninterface Renderable {\n    public function render(): string;\n}\n}";
        let class_src =
            "<?php\nuse App\\Contracts\\Renderable;\nclass View implements Renderable {\n}";
        let all_docs = vec![
            (
                uri("/contracts/Renderable.php"),
                Arc::new(ParsedDoc::parse(iface_src.to_string())),
            ),
            (
                uri("/View.php"),
                Arc::new(ParsedDoc::parse(class_src.to_string())),
            ),
        ];
        let class_doc = ParsedDoc::parse(class_src.to_string());
        let actions = implement_missing_actions(
            class_src,
            &class_doc,
            &all_docs,
            full_range(),
            &uri("/View.php"),
            &HashMap::new(),
        );
        assert!(
            !actions.is_empty(),
            "expected action when interface is resolved through use import"
        );
        if let CodeActionOrCommand::CodeAction(action) = &actions[0] {
            let changes = action.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(
                edits[0].new_text.contains("function render()"),
                "stub should contain 'function render()', got: {}",
                edits[0].new_text
            );
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn use_import_resolution_picks_correct_interface_over_same_short_name() {
        // Two interfaces share the short name `Logger`; only the imported one's
        // methods should be stubbed.  Both use braced-namespace syntax so the
        // AST traversal can track the namespace prefix.
        let wrong_iface = "<?php\nnamespace Other {\ninterface Logger {\n    public function wrong(): void;\n}\n}";
        let right_iface = "<?php\nnamespace App\\Logging {\ninterface Logger {\n    public function log(string $msg): void;\n}\n}";
        let class_src = "<?php\nuse App\\Logging\\Logger;\nclass FileLogger implements Logger {\n}";
        let all_docs = vec![
            (
                uri("/other/Logger.php"),
                Arc::new(ParsedDoc::parse(wrong_iface.to_string())),
            ),
            (
                uri("/logging/Logger.php"),
                Arc::new(ParsedDoc::parse(right_iface.to_string())),
            ),
            (
                uri("/FileLogger.php"),
                Arc::new(ParsedDoc::parse(class_src.to_string())),
            ),
        ];
        let class_doc = ParsedDoc::parse(class_src.to_string());
        let imports = HashMap::from([("Logger".to_string(), "App\\Logging\\Logger".to_string())]);
        let actions = implement_missing_actions(
            class_src,
            &class_doc,
            &all_docs,
            full_range(),
            &uri("/FileLogger.php"),
            &imports,
        );
        assert!(!actions.is_empty(), "expected action");
        if let CodeActionOrCommand::CodeAction(action) = &actions[0] {
            let changes = action.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(
                edits[0].new_text.contains("function log("),
                "should stub the correct Logger's 'log' method, got: {}",
                edits[0].new_text
            );
            assert!(
                !edits[0].new_text.contains("function wrong("),
                "should NOT stub the wrong Logger's 'wrong' method"
            );
        } else {
            panic!("expected CodeAction");
        }
    }
}
