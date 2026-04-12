/// Code action: "Promote constructor parameters" — converts property declarations
/// plus `$this->x = $x` constructor assignments to PHP 8.0 constructor property
/// promotion syntax.
///
/// Before:
/// ```php
/// class Foo {
///     private string $name;
///     public function __construct(string $name) {
///         $this->name = $name;
///     }
/// }
/// ```
///
/// After:
/// ```php
/// class Foo {
///     public function __construct(private string $name) {}
/// }
/// ```
use std::collections::HashMap;

use php_ast::{ClassMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind, Visibility};
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Url, WorkspaceEdit,
};

use crate::ast::{ParsedDoc, offset_to_position};

// ── Public entry point ────────────────────────────────────────────────────────

pub fn promote_constructor_actions(
    source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Url,
) -> Vec<CodeActionOrCommand> {
    let mut out = Vec::new();
    collect_promote(&doc.program().stmts, source, range, uri, &mut out);
    out
}

// ── Internal ──────────────────────────────────────────────────────────────────

/// Describes a property/param pair that can be promoted.
struct Promotion {
    /// Property member span — used to remove the whole line.
    prop_span_start: u32,
    prop_span_end: u32,
    /// Constructor param span start — we insert the visibility prefix here.
    param_span_start: u32,
    /// Visibility modifier to prepend to the param.
    visibility: &'static str,
    /// Whether to also insert `readonly `.
    is_readonly: bool,
    /// Assignment statement span — used to remove the whole line.
    assign_span_start: u32,
    assign_span_end: u32,
}

fn collect_promote<'a>(
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

                // Find the constructor.
                let ctor_member = c.members.iter().find(|m| {
                    matches!(&m.kind, ClassMemberKind::Method(method) if method.name == "__construct")
                });
                let ctor_member = match ctor_member {
                    Some(m) => m,
                    None => continue,
                };
                let ctor = match &ctor_member.kind {
                    ClassMemberKind::Method(m) => m,
                    _ => continue,
                };
                let ctor_body = match &ctor.body {
                    Some(b) => b,
                    None => continue,
                };

                // Build a map from property name -> (member span start, member span end, visibility, is_readonly)
                // Only include non-static properties that have a visibility modifier.
                let mut prop_info: HashMap<&str, (u32, u32, &'static str, bool)> = HashMap::new();
                for member in c.members.iter() {
                    if let ClassMemberKind::Property(p) = &member.kind
                        && !p.is_static
                        && p.visibility.is_some()
                    {
                        let vis = match &p.visibility {
                            Some(Visibility::Private) => "private",
                            Some(Visibility::Protected) => "protected",
                            _ => "public",
                        };
                        prop_info.insert(
                            p.name,
                            (member.span.start, member.span.end, vis, p.is_readonly),
                        );
                    }
                }

                if prop_info.is_empty() {
                    continue;
                }

                // For each constructor param, check if:
                // 1. The param doesn't already have a visibility (not already promoted).
                // 2. There is a matching property in prop_info.
                // 3. There is a `$this->name = $name` assignment in the constructor body.
                let mut promotions: Vec<Promotion> = Vec::new();

                for param in ctor.params.iter() {
                    // Skip already-promoted params.
                    if param.visibility.is_some() {
                        continue;
                    }
                    let param_name = param.name;

                    // Check if there's a matching property.
                    let (prop_start, prop_end, vis, is_readonly) = match prop_info.get(param_name) {
                        Some(info) => *info,
                        None => continue,
                    };

                    // Search constructor body for `$this->paramName = $paramName`.
                    let assign_span = find_this_assign(source, ctor_body, param_name);
                    let (assign_start, assign_end) = match assign_span {
                        Some(s) => s,
                        None => continue,
                    };

                    promotions.push(Promotion {
                        prop_span_start: prop_start,
                        prop_span_end: prop_end,
                        param_span_start: param.span.start,
                        visibility: vis,
                        is_readonly,
                        assign_span_start: assign_start,
                        assign_span_end: assign_end,
                    });
                }

                if promotions.is_empty() {
                    continue;
                }

                let count = promotions.len();
                let title = if count == 1 {
                    "Promote constructor parameter".to_string()
                } else {
                    format!("Promote {count} constructor parameters")
                };

                if let Some(action) = build_action(source, uri, &promotions, &title) {
                    out.push(action);
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_promote(inner, source, range, uri, out);
                }
            }
            _ => {}
        }
    }
}

/// Search `stmts` for a statement that is a bare expression `$this->prop = $var`
/// where `prop == param_name` and `var == param_name`.
/// Returns (stmt_span_start, stmt_span_end) when found.
fn find_this_assign(source: &str, stmts: &[Stmt<'_, '_>], param_name: &str) -> Option<(u32, u32)> {
    for stmt in stmts {
        if let StmtKind::Expression(expr) = &stmt.kind
            && let ExprKind::Assign(assign) = &expr.kind
        {
            // LHS must be `$this->paramName`
            if let ExprKind::PropertyAccess(pa) = &assign.target.kind {
                let is_this =
                    matches!(&pa.object.kind, ExprKind::Variable(v) if v.as_str() == "this");
                let prop_src = source
                    .get(pa.property.span.start as usize..pa.property.span.end as usize)
                    .unwrap_or("");
                // RHS must be `$paramName`
                let rhs_matches =
                    matches!(&assign.value.kind, ExprKind::Variable(v) if v.as_str() == param_name);
                if is_this && prop_src == param_name && rhs_matches {
                    return Some((stmt.span.start, stmt.span.end));
                }
            }
        }
    }
    None
}

/// Build the code action with text edits.
fn build_action(
    source: &str,
    uri: &Url,
    promotions: &[Promotion],
    title: &str,
) -> Option<CodeActionOrCommand> {
    let mut edits: Vec<TextEdit> = Vec::new();

    for p in promotions {
        // 1. Remove the property declaration (the whole line including newline).
        let prop_remove_range = whole_line_range(source, p.prop_span_start, p.prop_span_end);
        edits.push(TextEdit {
            range: prop_remove_range,
            new_text: String::new(),
        });

        // 2. Insert `visibility ` (and optionally `readonly `) before the param.
        let insert_pos = offset_to_position(source, p.param_span_start);
        let prefix = if p.is_readonly {
            format!("{} readonly ", p.visibility)
        } else {
            format!("{} ", p.visibility)
        };
        edits.push(TextEdit {
            range: Range {
                start: insert_pos,
                end: insert_pos,
            },
            new_text: prefix,
        });

        // 3. Remove the `$this->prop = $param;` assignment (whole line including newline).
        let assign_remove_range = whole_line_range(source, p.assign_span_start, p.assign_span_end);
        edits.push(TextEdit {
            range: assign_remove_range,
            new_text: String::new(),
        });
    }

    // Sort edits in reverse order so that earlier offsets aren't invalidated by
    // later changes. (LSP clients are supposed to handle this, but being explicit helps.)
    edits.sort_by(|a, b| {
        b.range
            .start
            .line
            .cmp(&a.range.start.line)
            .then(b.range.start.character.cmp(&a.range.start.character))
    });

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), edits);

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: title.to_string(),
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// Return a `Range` that covers the full line(s) containing `[span_start, span_end]`,
/// including the trailing newline so the blank line is removed entirely.
fn whole_line_range(source: &str, span_start: u32, span_end: u32) -> Range {
    let start_off = span_start as usize;
    let end_off = (span_end as usize).min(source.len());

    // Walk backwards to find the start of the line.
    let line_start = source[..start_off].rfind('\n').map(|i| i + 1).unwrap_or(0);

    // Walk forward to include the trailing newline.
    let line_end = if end_off < source.len() && source.as_bytes()[end_off] == b'\n' {
        end_off + 1
    } else {
        // No trailing newline — just use a byte scan to end of the current line.
        source[end_off..]
            .find('\n')
            .map(|i| end_off + i + 1)
            .unwrap_or(source.len())
    };

    Range {
        start: offset_to_position(source, line_start as u32),
        end: offset_to_position(source, line_end as u32),
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
    fn promotes_simple_private_property() {
        let src = "<?php\nclass Foo {\n    private string $name;\n    public function __construct(string $name) {\n        $this->name = $name;\n    }\n}\n";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = promote_constructor_actions(src, &doc, full_range(), &uri());
        assert!(!actions.is_empty(), "expected a promote action");
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            assert!(a.title.contains("Promote"));
            let edits = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let all_edits: Vec<&TextEdit> = edits.values().flat_map(|v| v.iter()).collect();
            // Should insert `private ` before the param.
            assert!(
                all_edits.iter().any(|e| e.new_text == "private "),
                "should insert 'private ' prefix, got: {:?}",
                all_edits.iter().map(|e| &e.new_text).collect::<Vec<_>>()
            );
            // Should have two deletion edits (property line + assignment line).
            assert!(
                all_edits.iter().filter(|e| e.new_text.is_empty()).count() >= 2,
                "should have at least 2 deletion edits"
            );
        }
    }

    #[test]
    fn promotes_readonly_property() {
        let src = "<?php\nclass Bar {\n    private readonly string $id;\n    public function __construct(string $id) {\n        $this->id = $id;\n    }\n}\n";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = promote_constructor_actions(src, &doc, full_range(), &uri());
        assert!(
            !actions.is_empty(),
            "expected a promote action for readonly"
        );
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            let edits = a.edit.as_ref().unwrap().changes.as_ref().unwrap();
            let all_edits: Vec<&TextEdit> = edits.values().flat_map(|v| v.iter()).collect();
            // Should insert `private readonly ` before the param.
            assert!(
                all_edits.iter().any(|e| e.new_text == "private readonly "),
                "should insert 'private readonly ' prefix, got: {:?}",
                all_edits.iter().map(|e| &e.new_text).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn no_action_when_no_constructor() {
        let src = "<?php\nclass Foo {\n    private string $name;\n}\n";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = promote_constructor_actions(src, &doc, full_range(), &uri());
        assert!(actions.is_empty(), "no action when no constructor exists");
    }

    #[test]
    fn no_action_when_no_matching_assignment() {
        // Property exists but constructor doesn't assign it via $this->name = $name.
        let src = "<?php\nclass Foo {\n    private string $name;\n    public function __construct(string $name) {\n        $this->name = strtolower($name);\n    }\n}\n";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = promote_constructor_actions(src, &doc, full_range(), &uri());
        assert!(
            actions.is_empty(),
            "no action when assignment is not a simple variable copy"
        );
    }

    #[test]
    fn no_action_when_already_promoted() {
        let src =
            "<?php\nclass Foo {\n    public function __construct(private string $name) {}\n}\n";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = promote_constructor_actions(src, &doc, full_range(), &uri());
        assert!(
            actions.is_empty(),
            "no action when param is already promoted"
        );
    }

    #[test]
    fn promotes_multiple_properties() {
        let src = "<?php\nclass Baz {\n    private string $name;\n    protected int $age;\n    public function __construct(string $name, int $age) {\n        $this->name = $name;\n        $this->age = $age;\n    }\n}\n";
        let doc = ParsedDoc::parse(src.to_string());
        let actions = promote_constructor_actions(src, &doc, full_range(), &uri());
        assert!(
            !actions.is_empty(),
            "expected a promote action for multiple props"
        );
        if let CodeActionOrCommand::CodeAction(a) = &actions[0] {
            assert!(
                a.title.contains('2') || a.title.contains("2"),
                "title should mention 2 promotions, got: {}",
                a.title
            );
        }
    }
}
