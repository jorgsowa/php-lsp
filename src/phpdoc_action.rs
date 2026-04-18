/// Code action: generate a PHPDoc stub for a function or method that lacks one.
use php_ast::{ClassMemberKind, EnumMemberKind, NamespaceBody, Param, Stmt, StmtKind};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Position, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ast::{ParsedDoc, format_type_hint, offset_to_position};
use crate::docblock::docblock_before;

/// Return "Generate PHPDoc" code actions for any function/method whose declaration line
/// falls within `range` and does not already have a docblock.
pub fn phpdoc_actions(
    uri: &Url,
    doc: &ParsedDoc,
    source: &str,
    range: Range,
) -> Vec<CodeActionOrCommand> {
    let line_starts = doc.line_starts();
    let mut actions = Vec::new();
    collect(
        &doc.program().stmts,
        uri,
        source,
        line_starts,
        range,
        &mut actions,
    );
    actions
}

fn collect(
    stmts: &[Stmt<'_, '_>],
    uri: &Url,
    source: &str,
    line_starts: &[u32],
    range: Range,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                let fn_line = offset_to_position(source, line_starts, stmt.span.start).line;
                if line_in_range(fn_line, range)
                    && docblock_before(source, stmt.span.start).is_none()
                {
                    let ret = f.return_type.as_ref().map(|t| format_type_hint(t));
                    if let Some(action) = make_action(uri, source, fn_line, &f.params, ret) {
                        out.push(action);
                    }
                }
            }
            StmtKind::Class(c) => {
                for member in c.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let fn_line =
                            offset_to_position(source, line_starts, member.span.start).line;
                        if line_in_range(fn_line, range)
                            && docblock_before(source, member.span.start).is_none()
                        {
                            let ret = m.return_type.as_ref().map(|t| format_type_hint(t));
                            if let Some(action) = make_action(uri, source, fn_line, &m.params, ret)
                            {
                                out.push(action);
                            }
                        }
                    }
                }
            }
            StmtKind::Trait(t) => {
                for member in t.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind {
                        let fn_line =
                            offset_to_position(source, line_starts, member.span.start).line;
                        if line_in_range(fn_line, range)
                            && docblock_before(source, member.span.start).is_none()
                        {
                            let ret = m.return_type.as_ref().map(|t| format_type_hint(t));
                            if let Some(action) = make_action(uri, source, fn_line, &m.params, ret)
                            {
                                out.push(action);
                            }
                        }
                    }
                }
            }
            StmtKind::Enum(e) => {
                for member in e.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind {
                        let fn_line =
                            offset_to_position(source, line_starts, member.span.start).line;
                        if line_in_range(fn_line, range)
                            && docblock_before(source, member.span.start).is_none()
                        {
                            let ret = m.return_type.as_ref().map(|t| format_type_hint(t));
                            if let Some(action) = make_action(uri, source, fn_line, &m.params, ret)
                            {
                                out.push(action);
                            }
                        }
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect(inner, uri, source, line_starts, range, out);
                }
            }
            _ => {}
        }
    }
}

fn line_in_range(line: u32, range: Range) -> bool {
    line >= range.start.line && line <= range.end.line
}

fn make_action(
    uri: &Url,
    source: &str,
    fn_line: u32,
    params: &[Param<'_, '_>],
    return_type: Option<String>,
) -> Option<CodeActionOrCommand> {
    let indent = source
        .lines()
        .nth(fn_line as usize)
        .map(|line| {
            let n = line.len() - line.trim_start().len();
            &line[..n]
        })
        .unwrap_or("")
        .to_string();

    let mut lines: Vec<String> = vec![format!("{indent}/**")];

    for p in params.iter() {
        let type_part = p
            .type_hint
            .as_ref()
            .map(|t| format!("{} ", format_type_hint(t)))
            .unwrap_or_default();
        let name = format!("${}", p.name);
        lines.push(format!("{indent} * @param {type_part}{name}"));
    }

    if let Some(ret) = return_type {
        lines.push(format!("{indent} * @return {ret}"));
    }

    lines.push(format!("{indent} */"));

    let new_text = lines.join("\n") + "\n";
    let pos = Position {
        line: fn_line,
        character: 0,
    };
    let edit = TextEdit {
        range: Range {
            start: pos,
            end: pos,
        },
        new_text,
    };

    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), vec![edit]);

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Generate PHPDoc".to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> ParsedDoc {
        ParsedDoc::parse(src.to_string())
    }

    fn uri() -> Url {
        Url::parse("file:///tmp/test.php").unwrap()
    }

    fn point(line: u32) -> Range {
        Range {
            start: Position { line, character: 0 },
            end: Position { line, character: 0 },
        }
    }

    #[test]
    fn generates_action_for_undocumented_function() {
        let src = "<?php\nfunction greet(string $name): string {}";
        let d = doc(src);
        let actions = phpdoc_actions(&uri(), &d, src, point(1));
        assert_eq!(actions.len(), 1);
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            assert_eq!(a.title, "Generate PHPDoc");
            let changes = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(edits[0].new_text.contains("@param string $name"));
            assert!(edits[0].new_text.contains("@return string"));
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn no_action_when_docblock_exists() {
        let src = "<?php\n/** Greets someone. */\nfunction greet(string $name): string {}";
        let d = doc(src);
        let actions = phpdoc_actions(&uri(), &d, src, point(2));
        assert!(
            actions.is_empty(),
            "should not offer action when docblock exists"
        );
    }

    #[test]
    fn no_action_when_cursor_not_on_function() {
        let src = "<?php\nfunction greet(string $name): string {}";
        let d = doc(src);
        let actions = phpdoc_actions(&uri(), &d, src, point(5));
        assert!(actions.is_empty());
    }

    #[test]
    fn generates_action_for_method_without_params() {
        let src = "<?php\nclass Foo {\n    public function bar(): void {}\n}";
        let d = doc(src);
        let actions = phpdoc_actions(&uri(), &d, src, point(2));
        assert_eq!(actions.len(), 1);
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let changes = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(edits[0].new_text.contains("@return void"));
            assert!(!edits[0].new_text.contains("@param"));
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn generates_action_for_trait_method() {
        let src = "<?php\ntrait Logger {\n    public function log(string $msg): void {}\n}";
        let d = doc(src);
        let actions = phpdoc_actions(&uri(), &d, src, point(2));
        assert_eq!(actions.len(), 1, "expected 1 action for trait method");
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let changes = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(edits[0].new_text.contains("@param string $msg"));
            assert!(edits[0].new_text.contains("@return void"));
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn generates_action_for_enum_method() {
        let src = "<?php\nenum Suit {\n    case Hearts;\n    public function label(int $pad): string { return ''; }\n}";
        let d = doc(src);
        let actions = phpdoc_actions(&uri(), &d, src, point(3));
        assert_eq!(actions.len(), 1, "expected 1 action for enum method");
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let changes = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(edits[0].new_text.contains("@param int $pad"));
            assert!(edits[0].new_text.contains("@return string"));
        } else {
            panic!("expected CodeAction");
        }
    }

    #[test]
    fn no_action_for_trait_method_with_existing_docblock() {
        let src = "<?php\ntrait Logger {\n    /** Already documented. */\n    public function log(string $msg): void {}\n}";
        let d = doc(src);
        let actions = phpdoc_actions(&uri(), &d, src, point(3));
        assert!(
            actions.is_empty(),
            "should not offer action when docblock exists"
        );
    }

    #[test]
    fn preserves_indentation() {
        let src = "<?php\nclass Foo {\n    public function bar(): void {}\n}";
        let d = doc(src);
        let actions = phpdoc_actions(&uri(), &d, src, point(2));
        assert_eq!(actions.len(), 1);
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let changes = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let edits = changes.values().next().unwrap();
            assert!(
                edits[0].new_text.starts_with("    /**"),
                "expected 4-space indent"
            );
        } else {
            panic!("expected CodeAction");
        }
    }
}
