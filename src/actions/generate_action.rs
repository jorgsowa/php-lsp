/// Code actions: "Generate constructor" and "Generate getters/setters".
use std::collections::{HashMap, HashSet};

use php_ast::{ClassMemberKind, NamespaceBody, Stmt, StmtKind};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView, format_type_hint};

pub fn generate_constructor_actions(
    _source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let mut out = Vec::new();
    collect_constructor(&doc.program().stmts, sv, range, uri, &mut out);
    out
}

pub fn generate_getters_setters_actions(
    _source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let mut out = Vec::new();
    collect_getters_setters(&doc.program().stmts, sv, range, uri, &mut out);
    out
}

struct Prop {
    name: String,
    type_str: Option<String>,
    is_readonly: bool,
}

fn collect_constructor<'a>(
    stmts: &[Stmt<'a, 'a>],
    sv: SourceView<'_>,
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

                // Skip if constructor already exists.
                let has_ctor = c.body.members.iter().any(|m| {
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
                push_action(sv, stmt.span.end, text, "Generate constructor", uri, out);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_constructor(&inner.stmts, sv, range, uri, out);
                }
            }
            _ => {}
        }
    }
}

fn collect_getters_setters<'a>(
    stmts: &[Stmt<'a, 'a>],
    sv: SourceView<'_>,
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

                let props = non_static_props(c);
                if props.is_empty() {
                    continue;
                }

                let mut text = String::new();
                let mut count = 0usize;
                let mut generated_getter = false;
                let mut generated_setter = false;
                for p in &props {
                    let cap = capitalize(&p.name);

                    let getter = format!("get{cap}");
                    let has_getter = existing.contains(&getter);
                    // A readonly property can only ever be assigned once, from
                    // inside its declaring class's constructor — a public
                    // setter would be a PHP fatal error ("Cannot modify
                    // readonly property") on the second call, so never
                    // generate one.
                    let setter = format!("set{cap}");
                    let has_setter = p.is_readonly || existing.contains(&setter);

                    if has_getter && has_setter {
                        continue;
                    }

                    // Count properties needing at least one accessor, not individual methods.
                    count += 1;

                    if !has_getter {
                        let ret = p
                            .type_str
                            .as_deref()
                            .map(|t| format!(": {t}"))
                            .unwrap_or_default();
                        text.push_str(&format!(
                            "    public function {getter}(){ret}\n    {{\n        return $this->{};\n    }}\n\n",
                            p.name
                        ));
                        generated_getter = true;
                    }

                    if !has_setter {
                        let param = match &p.type_str {
                            Some(t) => format!("{t} ${}", p.name),
                            None => format!("${}", p.name),
                        };
                        text.push_str(&format!(
                            "    public function {setter}({param}): void\n    {{\n        $this->{n} = ${n};\n    }}\n\n",
                            n = p.name
                        ));
                        generated_setter = true;
                    }
                }

                if count == 0 {
                    continue;
                }

                let title = if count == 1 {
                    match (generated_getter, generated_setter) {
                        (true, false) => "Generate getter".to_string(),
                        (false, true) => "Generate setter".to_string(),
                        _ => "Generate getter/setter".to_string(),
                    }
                } else {
                    format!("Generate {count} getters/setters")
                };
                push_action(sv, stmt.span.end, text, &title, uri, out);
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_getters_setters(&inner.stmts, sv, range, uri, out);
                }
            }
            _ => {}
        }
    }
}

fn non_static_props(c: &php_ast::ClassDecl<'_, '_>) -> Vec<Prop> {
    // A `readonly class` makes every property readonly even when the
    // property itself carries no `readonly` keyword of its own.
    let class_is_readonly = c.modifiers.is_readonly;

    let mut props: Vec<Prop> = c
        .body
        .members
        .iter()
        .filter_map(|m| {
            if let ClassMemberKind::Property(p) = &m.kind
                && !p.is_static
            {
                return Some(Prop {
                    name: p.name.to_string(),
                    type_str: p.type_hint.as_ref().map(format_type_hint),
                    is_readonly: p.is_readonly || class_is_readonly,
                });
            }
            None
        })
        .collect();

    // Also collect constructor-promoted properties.
    if let Some(ctor) = c.body.members.iter().find_map(|m| {
        if let ClassMemberKind::Method(method) = &m.kind
            && method.name == "__construct"
        {
            Some(method)
        } else {
            None
        }
    }) {
        for p in ctor.params.iter() {
            if p.visibility.is_some() {
                props.push(Prop {
                    name: p.name.to_string(),
                    type_str: p.type_hint.as_ref().map(format_type_hint),
                    is_readonly: p.is_readonly || class_is_readonly,
                });
            }
        }
    }

    props
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
    sv: SourceView<'_>,
    class_end_offset: u32,
    new_text: String,
    title: &str,
    uri: &Uri,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let closing_line = sv.position_of(class_end_offset.saturating_sub(1)).line;
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
