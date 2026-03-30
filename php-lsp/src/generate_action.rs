/// Code actions: "Generate constructor" and "Generate getters/setters".
use std::collections::{HashMap, HashSet};

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ast::{ParsedDoc, format_type_hint, offset_to_position};

// ── Public entry points ───────────────────────────────────────────────────────

pub fn generate_constructor_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    let mut out = Vec::new();
    collect_constructor(&doc.program().stmts, source, range, uri, &mut out);
    out
}

pub fn generate_getters_setters_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    let mut out = Vec::new();
    collect_getters_setters(&doc.program().stmts, source, range, uri, &mut out);
    out
}

// ── Internal ──────────────────────────────────────────────────────────────────

struct Prop {
    name: String,
    type_str: Option<String>,
}

fn collect_constructor<'a>(
    stmts: &[Stmt<'a, 'a>],
    source: &str,
    range: Range,
    uri: &Url,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let class_start = offset_to_position(source, stmt.span.start).line;
                let class_end = offset_to_position(source, stmt.span.end).line;
                if class_start > range.end.line || class_end < range.start.line {
                    continue;
                }

                // Skip if constructor already exists.
                let has_ctor = c.members.iter().any(|m| {
                    matches!(&m.kind, ClassMemberKind::Method(method) if method.name == "__construct")
                });
                if has_ctor {
                    continue;
                }

                let props = non_static_props(c);
                if props.is_empty() {
                    continue;
                }

                let text = generate_constructor_text(&props);
                push_action(
                    source,
                    stmt.span.end,
                    text,
                    "Generate constructor",
                    uri,
                    out,
                );
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_constructor(inner, source, range, uri, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_getters_setters<'a>(
    stmts: &[Stmt<'a, 'a>],
    source: &str,
    range: Range,
    uri: &Url,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Class(c) => {
                let class_start = offset_to_position(source, stmt.span.start).line;
                let class_end = offset_to_position(source, stmt.span.end).line;
                if class_start > range.end.line || class_end < range.start.line {
                    continue;
                }

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

                let props = non_static_props(c);
                if props.is_empty() {
                    continue;
                }

                let mut text = String::new();
                let mut count = 0usize;
                for p in &props {
                    let cap = capitalize(&p.name);

                    let getter = format!("get{cap}");
                    if !existing.contains(&getter) {
                        let ret = p
                            .type_str
                            .as_deref()
                            .map(|t| format!(": {t}"))
                            .unwrap_or_default();
                        text.push_str(&format!(
                            "    public function {getter}(){ret}\n    {{\n        return $this->{};\n    }}\n\n",
                            p.name
                        ));
                        count += 1;
                    }

                    let setter = format!("set{cap}");
                    if !existing.contains(&setter) {
                        let param = match &p.type_str {
                            Some(t) => format!("{t} ${}", p.name),
                            None => format!("${}", p.name),
                        };
                        text.push_str(&format!(
                            "    public function {setter}({param}): void\n    {{\n        $this->{n} = ${n};\n    }}\n\n",
                            n = p.name
                        ));
                        count += 1;
                    }
                }

                if count == 0 {
                    continue;
                }

                let title = if count == 1 {
                    "Generate getter/setter".to_string()
                } else {
                    format!("Generate {count} getters/setters")
                };
                push_action(source, stmt.span.end, text, &title, uri, out);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_getters_setters(inner, source, range, uri, out);
                }
            }
            _ => {}
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn non_static_props(c: &php_ast::ClassDecl<'_, '_>) -> Vec<Prop> {
    c.members
        .iter()
        .filter_map(|m| {
            if let ClassMemberKind::Property(p) = &m.kind
                && !p.is_static
            {
                return Some(Prop {
                    name: p.name.to_string(),
                    type_str: p.type_hint.as_ref().map(format_type_hint),
                });
            }
            None
        })
        .collect()
}

fn generate_constructor_text(props: &[Prop]) -> String {
    let mut text = String::from("    public function __construct(\n");
    for p in props {
        match &p.type_str {
            Some(t) => text.push_str(&format!("        {t} ${},\n", p.name)),
            None => text.push_str(&format!("        ${},\n", p.name)),
        }
    }
    text.push_str("    ) {\n");
    for p in props {
        text.push_str(&format!("        $this->{n} = ${n};\n", n = p.name));
    }
    text.push_str("    }\n\n");
    text
}

fn push_action(
    source: &str,
    class_end_offset: u32,
    new_text: String,
    title: &str,
    uri: &Url,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let closing_line = offset_to_position(source, class_end_offset.saturating_sub(1)).line;
    let pos = Position {
        line: closing_line,
        character: 0,
    };
    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: pos,
                end: pos,
            },
            new_text,
        }],
    );
    out.push(CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }));
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

    fn uri() -> Url {
        Url::parse("file:///test.php").unwrap()
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
    fn generates_constructor_for_class_with_properties() {
        let src = "<?php\nclass User {\n    private string $name;\n    private int $age;\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = generate_constructor_actions(src, &doc, full_range(), &uri());
        assert!(
            !actions.is_empty(),
            "expected a generate constructor action"
        );
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let edits = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let text = &edits.values().next().unwrap()[0].new_text;
            assert!(text.contains("__construct"), "should contain __construct");
            assert!(text.contains("$this->name = $name"), "should assign name");
            assert!(text.contains("$this->age = $age"), "should assign age");
            assert!(text.contains("string $name"), "should include type hint");
        }
    }

    #[test]
    fn no_constructor_action_when_constructor_exists() {
        let src = "<?php\nclass User {\n    private string $name;\n    public function __construct(string $name) { $this->name = $name; }\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = generate_constructor_actions(src, &doc, full_range(), &uri());
        assert!(
            actions.is_empty(),
            "no action when constructor already exists"
        );
    }

    #[test]
    fn no_constructor_action_for_class_without_properties() {
        let src = "<?php\nclass Empty {}";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = generate_constructor_actions(src, &doc, full_range(), &uri());
        assert!(actions.is_empty(), "no action for class with no properties");
    }

    #[test]
    fn generates_getters_and_setters() {
        let src = "<?php\nclass User {\n    private string $name;\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = generate_getters_setters_actions(src, &doc, full_range(), &uri());
        assert!(!actions.is_empty(), "expected getter/setter action");
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let edits = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let text = &edits.values().next().unwrap()[0].new_text;
            assert!(text.contains("getName"), "should contain getter");
            assert!(text.contains("setName"), "should contain setter");
            assert!(
                text.contains("return $this->name"),
                "getter should return property"
            );
        }
    }

    #[test]
    fn skips_existing_getter_setter() {
        let src = "<?php\nclass User {\n    private string $name;\n    public function getName(): string { return $this->name; }\n}";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = generate_getters_setters_actions(src, &doc, full_range(), &uri());
        if let Some(CodeActionOrCommand::CodeAction(a)) = actions.first() {
            let edits = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let text = &edits.values().next().unwrap()[0].new_text;
            assert!(
                !text.contains("getName"),
                "should not regenerate existing getter"
            );
            assert!(
                text.contains("setName"),
                "should still generate missing setter"
            );
        }
    }
}
