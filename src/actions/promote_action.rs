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
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView};

pub fn promote_constructor_actions(
    _source: &str,
    doc: &ParsedDoc,
    range: Range,
    uri: &Uri,
) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let mut out = Vec::new();
    collect_promote(&doc.program().stmts, sv, range, uri, &mut out);
    out
}

/// Describes a property/param pair that can be promoted.
struct Promotion {
    /// Property member span — used to remove the whole line.
    prop_span_start: u32,
    prop_span_end: u32,
    /// Constructor param span start — we insert the visibility prefix here.
    param_span_start: u32,
    /// Constructor param span end — we insert the property's default value
    /// here, when the param doesn't already declare its own.
    param_span_end: u32,
    /// Visibility modifier to prepend to the param.
    visibility: &'static str,
    /// Whether to also insert `readonly `.
    is_readonly: bool,
    /// Property type hint string (e.g., "string", "?int"). `None` here when
    /// the constructor parameter already declares a type — emitting the
    /// property's type in that case would produce `private string string $x`.
    type_hint: Option<String>,
    /// Property default value source text (e.g., "3", "'foo'"). `None` here
    /// when the constructor parameter already declares its own default —
    /// carried over so promoting a property with `= 3` doesn't silently drop
    /// the default (the promoted param would take `null`/nothing instead).
    default: Option<String>,
    /// Assignment statement span — used to remove the whole line.
    assign_span_start: u32,
    assign_span_end: u32,
}

fn collect_promote<'a>(
    stmts: &[Stmt<'a, 'a>],
    sv: SourceView<'_>,
    range: Range,
    uri: &Uri,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            // Note: Only handles braced namespaces (namespace Foo { ... }).
            // Unbraced namespaces (namespace Foo;) don't nest statements, so promotion
            // works for classes in unbraced namespaces at the top level.
            // However, classes in unbraced namespaces after namespace declarations
            // would require tracking namespace context across statements, which isn't
            // done here. This is a known limitation.
            StmtKind::Class(c) => {
                let class_start = sv.position_of(stmt.span.start).line;
                let class_end = sv.position_of(stmt.span.end).line;
                if class_start > range.end.line || class_end < range.start.line {
                    continue;
                }

                // Find the constructor.
                let ctor_member = c.body.members.iter().find(|m| {
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

                // Build a map from property name -> (member span start, member span end, visibility, is_readonly, type_hint, default text)
                // Only include non-static properties that have a visibility modifier.
                type PropInfo = (u32, u32, &'static str, bool, Option<String>, Option<String>);
                let mut prop_info: HashMap<String, PropInfo> = HashMap::new();
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Property(p) = &member.kind
                        && !p.is_static
                        && p.visibility.is_some()
                    {
                        let vis = match &p.visibility {
                            Some(Visibility::Private) => "private",
                            Some(Visibility::Protected) => "protected",
                            _ => "public",
                        };
                        let type_hint = p
                            .type_hint
                            .as_ref()
                            .map(|t| crate::document::ast::format_type_hint(t));
                        let default = p.default.as_ref().map(|d| {
                            sv.source()[d.span.start as usize..d.span.end as usize].to_string()
                        });
                        prop_info.insert(
                            p.name.to_string(),
                            (
                                member.span.start,
                                member.span.end,
                                vis,
                                p.is_readonly,
                                type_hint,
                                default,
                            ),
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
                    let (prop_start, prop_end, vis, is_readonly, type_hint, default) =
                        match prop_info.get(param_name.to_string().as_str()) {
                            Some(info) => info.clone(),
                            None => continue,
                        };

                    // Search constructor body for `$this->paramName = $paramName`.
                    let assign_span =
                        find_this_assign(sv.source(), &ctor_body.stmts, &param_name.to_string());
                    let (assign_start, assign_end) = match assign_span {
                        Some(s) => s,
                        None => continue,
                    };

                    // If the constructor param already declares a type, don't
                    // re-emit the property's type — `private string string $x`
                    // is a parse error.
                    let effective_type_hint = if param.type_hint.is_some() {
                        None
                    } else {
                        type_hint
                    };

                    // Same precedence as the type hint: a param that already
                    // declares its own default keeps it untouched.
                    let effective_default = if param.default.is_some() {
                        None
                    } else {
                        default
                    };

                    promotions.push(Promotion {
                        prop_span_start: prop_start,
                        prop_span_end: prop_end,
                        param_span_start: param.span.start,
                        param_span_end: param.span.end,
                        visibility: vis,
                        is_readonly,
                        type_hint: effective_type_hint,
                        default: effective_default,
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

                if let Some(action) = build_action(sv, uri, &promotions, &title) {
                    out.push(action);
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_promote(&inner.stmts, sv, range, uri, out);
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
    sv: SourceView<'_>,
    uri: &Uri,
    promotions: &[Promotion],
    title: &str,
) -> Option<CodeActionOrCommand> {
    let mut edits: Vec<TextEdit> = Vec::new();

    for p in promotions {
        // 1. Remove the property declaration (the whole line including newline).
        let prop_remove_range = whole_line_range(sv, p.prop_span_start, p.prop_span_end);
        edits.push(TextEdit {
            range: prop_remove_range,
            new_text: String::new(),
        });

        // 2. Insert `visibility [type_hint] [readonly] ` before the param.
        let insert_pos = sv.position_of(p.param_span_start);
        let prefix = match (&p.type_hint, p.is_readonly) {
            (Some(th), true) => format!("{} {} readonly ", p.visibility, th),
            (Some(th), false) => format!("{} {} ", p.visibility, th),
            (None, true) => format!("{} readonly ", p.visibility),
            (None, false) => format!("{} ", p.visibility),
        };
        edits.push(TextEdit {
            range: Range {
                start: insert_pos,
                end: insert_pos,
            },
            new_text: prefix,
        });

        // 2b. Carry over the property's default value, when the param
        // doesn't already declare its own — otherwise it's silently dropped.
        if let Some(default) = &p.default {
            let end_pos = sv.position_of(p.param_span_end);
            edits.push(TextEdit {
                range: Range {
                    start: end_pos,
                    end: end_pos,
                },
                new_text: format!(" = {default}"),
            });
        }

        // 3. Remove the `$this->prop = $param;` assignment (whole line including newline).
        let assign_remove_range = whole_line_range(sv, p.assign_span_start, p.assign_span_end);
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
fn whole_line_range(sv: SourceView<'_>, span_start: u32, span_end: u32) -> Range {
    let start_off = span_start as usize;
    let end_off = (span_end as usize).min(sv.source().len());

    // Walk backwards to find the start of the line.
    let line_start = sv.source()[..start_off]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);

    // Walk forward to include the trailing newline.
    let line_end = if end_off < sv.source().len() && sv.source().as_bytes()[end_off] == b'\n' {
        end_off + 1
    } else {
        // No trailing newline — just use a byte scan to end of the current line.
        sv.source()[end_off..]
            .find('\n')
            .map(|i| end_off + i + 1)
            .unwrap_or(sv.source().len())
    };

    Range {
        start: sv.position_of(line_start as u32),
        end: sv.position_of(line_end as u32),
    }
}
