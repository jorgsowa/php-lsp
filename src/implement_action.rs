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

use crate::ast::{ParsedDoc, format_type_hint, offset_to_position};
use crate::hover::format_params_str;

struct MethodStub {
    name: String,
    visibility: &'static str,
    is_static: bool,
    params: String,
    return_type: Option<String>,
}

pub fn implement_missing_actions(
    source: &str,
    doc: &ParsedDoc,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    range: Range,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    collect_actions(&doc.program().stmts, source, all_docs, range, uri, &mut actions);
    actions
}

fn collect_actions(
    stmts: &[Stmt<'_, '_>],
    source: &str,
    all_docs: &[(Url, Arc<ParsedDoc>)],
    range: Range,
    uri: &Url,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                // Only consider classes whose declaration overlaps the requested range.
                let class_start = offset_to_position(source, stmt.span.start).line;
                let class_end = offset_to_position(source, stmt.span.end).line;
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
                    for stub in abstract_methods_of(&short, all_docs) {
                        if !existing.contains(&stub.name) {
                            missing.push(stub);
                        }
                    }
                }

                // Abstract parent class (if any).
                if let Some(parent) = &c.extends {
                    let parent_name = parent.to_string_repr().into_owned();
                    let short = last_segment(&parent_name).to_string();
                    for stub in abstract_methods_of(&short, all_docs) {
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
                let closing_line =
                    offset_to_position(source, stmt.span.end.saturating_sub(1)).line;
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
                    collect_actions(inner, source, all_docs, range, uri, out);
                }
            }
            _ => {}
        }
    }
}

/// Collect abstract/interface methods declared by `name` across all documents.
fn abstract_methods_of(name: &str, all_docs: &[(Url, Arc<ParsedDoc>)]) -> Vec<MethodStub> {
    for (_, doc) in all_docs {
        if let Some(stubs) = collect_abstract_methods(&doc.program().stmts, name) {
            return stubs;
        }
    }
    vec![]
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
                                return_type: method.return_type.as_ref().map(|t| format_type_hint(t)),
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
                if let NamespaceBody::Braced(inner) = &ns.body {
                    if let Some(stubs) = collect_abstract_methods(inner, name) {
                        return Some(stubs);
                    }
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
            "    {} {}function {}({}){ret}\n    {{\n        // TODO: implement\n    }}\n\n",
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
            start: Position { line: 0, character: 0 },
            end: Position { line: u32::MAX, character: u32::MAX },
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
        );
        assert!(actions.is_empty(), "no action needed when all methods are implemented");
    }

    #[test]
    fn generates_action_for_abstract_class_method() {
        let abstract_src = "<?php\nabstract class Shape {\n    abstract public function area(): float;\n}";
        let class_src = "<?php\nclass Circle extends Shape {\n}";
        let all_docs = vec![doc(abstract_src), doc(class_src)];
        let class_doc = ParsedDoc::parse(class_src.to_string());
        let actions = implement_missing_actions(
            class_src,
            &class_doc,
            &all_docs,
            full_range(),
            &uri("/b.php"),
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
}
