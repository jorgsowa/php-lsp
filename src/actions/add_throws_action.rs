/// Code action: add missing `@throws` tags to an existing PHPDoc.
///
/// Triggered when a function/method has a docblock but the body contains
/// `throw new ClassName()` expressions whose exception class is not yet
/// listed in any `@throws` tag.
use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;

use php_ast::{
    ClassMemberKind, EnumMemberKind, ExprKind, NamespaceBody, Stmt, StmtKind,
    visitor::{Visitor, walk_expr, walk_stmt},
};
use tower_lsp_server::ls_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, Range, TextEdit, Uri, WorkspaceEdit,
};

use crate::document::ast::{ParsedDoc, SourceView};
use crate::lang::docblock::parse_docblock;

/// Return "Add @throws …" actions for every function/method whose declaration
/// line falls within `range`, already has a docblock, and whose body contains
/// `throw new ClassName()` expressions not yet listed in `@throws`.
pub fn add_throws_actions(uri: &Uri, doc: &ParsedDoc, range: Range) -> Vec<CodeActionOrCommand> {
    let sv = doc.view();
    let mut out = Vec::new();
    collect_stmts(&doc.program().stmts, uri, sv, range, &mut out);
    out
}

fn collect_stmts<'a>(
    stmts: &[Stmt<'a, 'a>],
    uri: &Uri,
    sv: SourceView<'_>,
    range: Range,
    out: &mut Vec<CodeActionOrCommand>,
) {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) => {
                if line_in_range(sv.position_of(stmt.span.start).line, range)
                    && let Some(dc) = &f.doc_comment
                {
                    maybe_push(uri, sv, dc, f.body, None, None, out);
                }
            }
            StmtKind::Class(c) => {
                let self_name = c.name.map(|n| n.to_string());
                let parent_name = c.extends.as_ref().map(|p| p.to_string_repr().into_owned());
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && line_in_range(sv.position_of(member.span.start).line, range)
                        && let Some(dc) = &m.doc_comment
                        && let Some(body) = m.body
                    {
                        maybe_push(
                            uri,
                            sv,
                            dc,
                            body,
                            self_name.as_deref(),
                            parent_name.as_deref(),
                            out,
                        );
                    }
                }
            }
            StmtKind::Trait(t) => {
                let self_name = Some(t.name.to_string());
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && line_in_range(sv.position_of(member.span.start).line, range)
                        && let Some(dc) = &m.doc_comment
                        && let Some(body) = m.body
                    {
                        maybe_push(uri, sv, dc, body, self_name.as_deref(), None, out);
                    }
                }
            }
            StmtKind::Enum(e) => {
                let self_name = Some(e.name.to_string());
                for member in e.body.members.iter() {
                    if let EnumMemberKind::Method(m) = &member.kind
                        && line_in_range(sv.position_of(member.span.start).line, range)
                        && let Some(dc) = &m.doc_comment
                        && let Some(body) = m.body
                    {
                        maybe_push(uri, sv, dc, body, self_name.as_deref(), None, out);
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body {
                    collect_stmts(&inner.stmts, uri, sv, range, out);
                }
            }
            _ => {}
        }
    }
}

fn maybe_push(
    uri: &Uri,
    sv: SourceView<'_>,
    doc_comment: &php_ast::Comment<'_>,
    body: &php_ast::Block<'_, '_>,
    self_name: Option<&str>,
    parent_name: Option<&str>,
    out: &mut Vec<CodeActionOrCommand>,
) {
    let parsed = parse_docblock(doc_comment.text);
    if parsed.is_inherit_doc {
        return;
    }

    let existing: HashSet<&str> = parsed.throws.iter().map(|t| t.class.as_str()).collect();

    let mut collector = ThrowCollector {
        self_name: self_name.map(str::to_string),
        parent_name: parent_name.map(str::to_string),
        ..Default::default()
    };
    let _ = collector.visit_block(body);

    let mut missing: Vec<String> = collector
        .classes
        .into_iter()
        .filter(|c| !existing.contains(c.as_str()))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    if missing.is_empty() {
        return;
    }
    missing.sort();

    let source = sv.source();
    let doc_start_line = sv.position_of(doc_comment.span.start).line;
    let indent = extract_indent(source, doc_start_line);

    let text = doc_comment.text;
    let close_offset = text.rfind("*/").unwrap_or(text.len());

    // Multi-line docblock: insert right after the newline that precedes `*/`.
    // Single-line docblock: insert before `*/` and push `*/` to a new line.
    let (insert_byte, new_text) = match text[..close_offset].rfind('\n') {
        Some(nl_pos) => {
            let byte = doc_comment.span.start as usize + nl_pos + 1;
            let mut t = String::new();
            for class in &missing {
                t.push_str(&format!("{indent} * @throws {class}\n"));
            }
            (byte, t)
        }
        None => {
            // Single-line: insert after any space before `*/`.
            let byte = doc_comment.span.start as usize + close_offset;
            let mut t = String::new();
            for class in &missing {
                t.push_str(&format!("\n{indent} * @throws {class}"));
            }
            t.push_str(&format!("\n{indent} "));
            (byte, t)
        }
    };
    let insert_pos = sv.position_of(insert_byte as u32);

    let title = if missing.len() == 1 {
        format!("Add @throws {} to PHPDoc", missing[0])
    } else {
        "Add missing @throws tags to PHPDoc".to_string()
    };

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: insert_pos,
                end: insert_pos,
            },
            new_text,
        }],
    );

    out.push(CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }));
}

// ── Visitor ──────────────────────────────────────────────────────────────────

#[derive(Default)]
struct ThrowCollector {
    classes: Vec<String>,
    /// Enclosing class/trait/enum short name — resolves `self`/`static`.
    self_name: Option<String>,
    /// Enclosing class's `extends` name, if any — resolves `parent`.
    parent_name: Option<String>,
}

impl ThrowCollector {
    fn collect_from_expr(&mut self, expr: &php_ast::Expr<'_, '_>) {
        match &expr.kind {
            ExprKind::New(n) => {
                if let ExprKind::Identifier(name) = &n.class.kind {
                    // `self`/`static`/`parent` resolve to the real class name where
                    // known, so `@throws self` (nonsensical to a reader) isn't
                    // offered and dedup against an existing `@throws RealName`
                    // tag actually works.
                    let resolved = match name.as_str() {
                        "self" | "static" => self.self_name.clone(),
                        "parent" => self.parent_name.clone(),
                        _ => None,
                    };
                    self.classes
                        .push(resolved.unwrap_or_else(|| name.as_str().to_string()));
                }
            }
            ExprKind::Parenthesized(inner) => self.collect_from_expr(inner),
            _ => {}
        }
    }
}

impl<'arena, 'src> Visitor<'arena, 'src> for ThrowCollector {
    fn visit_stmt(&mut self, stmt: &php_ast::Stmt<'arena, 'src>) -> ControlFlow<()> {
        match &stmt.kind {
            StmtKind::Throw(expr) => {
                self.collect_from_expr(expr);
                ControlFlow::Continue(())
            }
            // Nested named declarations are separate scopes — don't cross.
            StmtKind::Function(_)
            | StmtKind::Class(_)
            | StmtKind::Trait(_)
            | StmtKind::Enum(_)
            | StmtKind::Interface(_) => ControlFlow::Continue(()),
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &php_ast::Expr<'arena, 'src>) -> ControlFlow<()> {
        match &expr.kind {
            // PHP 8+ throw-as-expression: `$x ?? throw new Foo()`
            ExprKind::ThrowExpr(inner) => {
                self.collect_from_expr(inner);
                ControlFlow::Continue(())
            }
            // Closures and arrow functions are separate scopes.
            ExprKind::Closure(_) | ExprKind::ArrowFunction(_) => ControlFlow::Continue(()),
            _ => walk_expr(self, expr),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn extract_indent(source: &str, line: u32) -> String {
    source
        .lines()
        .nth(line as usize)
        .map(|l| {
            let n = l.len() - l.trim_start().len();
            l[..n].to_string()
        })
        .unwrap_or_default()
}

fn line_in_range(line: u32, range: Range) -> bool {
    line >= range.start.line && line <= range.end.line
}
