use std::ops::ControlFlow;

use php_ast::visitor::{Visitor, walk_expr};
use php_ast::{Arg, ClassMemberKind, Expr, ExprKind, NamespaceBody, Param, Stmt, StmtKind};
use tower_lsp::lsp_types::Position;

use crate::document::ast::{ParsedDoc, format_type_hint};
use crate::text::fqn_short_name;

/// Resolve the class(es) of a named-argument call's receiver variable, for
/// looking up the method's parameter signature. `receiver_offset` is a byte
/// offset landing inside the receiver's variable token (see
/// `find_named_arg_at`), used to look up mir's recorded type there. Returns
/// short class names, `|`-joined for unions.
fn resolve_method_receiver_class(
    source: &str,
    doc: &ParsedDoc,
    position: Position,
    receiver_var: &str,
    receiver_offset: u32,
    analysis: Option<&mir_analyzer::FileAnalysis>,
) -> Option<String> {
    if let Some(a) = analysis
        && let Some(ty) = crate::types::type_query::type_at_offset(a, receiver_offset)
    {
        let names: Vec<String> = crate::types::type_query::class_names(ty)
            .iter()
            .map(|fqcn| fqn_short_name(fqcn).to_string())
            .collect();
        if !names.is_empty() {
            return Some(names.join("|"));
        }
    }
    if receiver_var == "$this" {
        return crate::types::type_map::enclosing_class_at(source, doc, position);
    }
    None
}

use super::formatting::{format_default_value, wrap_php};
use super::members::find_parent_class_name;

pub(crate) enum NamedArgCallee {
    Function(String),
    Method {
        receiver_var: String,
        receiver_offset: u32,
        method: String,
    },
    StaticMethod {
        class: String,
        method: String,
    },
}

/// The label at `arg.name`'s span containing `offset`, if any of `args` is a
/// named argument there.
fn label_at(args: &[Arg<'_, '_>], offset: u32) -> Option<String> {
    args.iter().find_map(|arg| {
        let name = arg.name.as_ref()?;
        let span = name.span();
        (span.start <= offset && offset < span.end).then(|| name.to_string_repr().into_owned())
    })
}

/// Find the named-argument call whose label span contains `offset`, walking
/// the AST rather than scanning source text — so a label on a call wrapped
/// across lines (`$m->send(\n    to: '...',\n)`) resolves the same as one on
/// a single line, and nested calls (`outer(a: inner(x: 1))`) resolve to the
/// innermost enclosing call.
pub(crate) fn find_named_arg_at(doc: &ParsedDoc, offset: u32) -> Option<(NamedArgCallee, String)> {
    struct Finder {
        offset: u32,
        result: Option<(NamedArgCallee, String)>,
    }

    impl<'arena, 'src> Visitor<'arena, 'src> for Finder {
        fn visit_expr(&mut self, expr: &Expr<'arena, 'src>) -> ControlFlow<()> {
            if self.result.is_some() {
                return ControlFlow::Break(());
            }
            match &expr.kind {
                ExprKind::FunctionCall(f) => {
                    if let Some(label) = label_at(&f.args, self.offset)
                        && let Some(name) = f.name.name_str()
                    {
                        self.result = Some((NamedArgCallee::Function(name.to_string()), label));
                        return ControlFlow::Break(());
                    }
                }
                ExprKind::MethodCall(m) | ExprKind::NullsafeMethodCall(m) => {
                    if let Some(label) = label_at(&m.args, self.offset)
                        && let ExprKind::Variable(recv) = &m.object.kind
                        && let Some(method) = m.method.name_str()
                    {
                        self.result = Some((
                            NamedArgCallee::Method {
                                receiver_var: format!("${}", recv.as_str()),
                                receiver_offset: m.object.span.start,
                                method: method.to_string(),
                            },
                            label,
                        ));
                        return ControlFlow::Break(());
                    }
                }
                ExprKind::StaticMethodCall(s) => {
                    if let Some(label) = label_at(&s.args, self.offset)
                        && let ExprKind::Identifier(class) = &s.class.kind
                        && let Some(method) = s.method.name_str()
                    {
                        let short = fqn_short_name(class.trim_start_matches('\\')).to_string();
                        self.result = Some((
                            NamedArgCallee::StaticMethod {
                                class: short,
                                method: method.to_string(),
                            },
                            label,
                        ));
                        return ControlFlow::Break(());
                    }
                }
                _ => {}
            }
            walk_expr(self, expr)
        }
    }

    let mut finder = Finder {
        offset,
        result: None,
    };
    for stmt in doc.program().stmts.iter() {
        let _ = finder.visit_stmt(stmt);
    }
    finder.result
}

/// Build the hover string for a named argument label.
///
/// Returns `None` when the callee or matching parameter cannot be found.
pub(crate) fn named_arg_hover_value(
    source: &str,
    doc: &ParsedDoc,
    other_docs: &[(
        tower_lsp::lsp_types::Url,
        std::sync::Arc<crate::document::ast::ParsedDoc>,
    )],
    position: Position,
    callee: &NamedArgCallee,
    label: &str,
    analysis: Option<&mir_analyzer::FileAnalysis>,
) -> Option<String> {
    let all_docs = || std::iter::once(doc).chain(other_docs.iter().map(|(_, d)| d.as_ref()));

    match callee {
        NamedArgCallee::Function(name) => {
            for d in all_docs() {
                if let Some((sig, db)) =
                    find_param_sig_in_stmts(d.source(), &d.program().stmts, name, None, label)
                {
                    return Some(format_named_param_hover(&sig, db.as_ref(), label));
                }
            }
            None
        }
        NamedArgCallee::Method {
            receiver_var,
            receiver_offset,
            method,
        } => {
            let class_name = resolve_method_receiver_class(
                source,
                doc,
                position,
                receiver_var,
                *receiver_offset,
                analysis,
            )?;
            let first_class = class_name
                .split('|')
                .next()
                .unwrap_or(&class_name)
                .to_owned();
            for d in all_docs() {
                if let Some((sig, db)) = find_param_sig_in_stmts(
                    d.source(),
                    &d.program().stmts,
                    method,
                    Some(&first_class),
                    label,
                ) {
                    return Some(format_named_param_hover(&sig, db.as_ref(), label));
                }
            }
            None
        }
        NamedArgCallee::StaticMethod { class, method } => {
            let effective_class = if class == "self" || class == "static" {
                crate::types::type_map::enclosing_class_at(source, doc, position)
                    .unwrap_or_else(|| class.clone())
            } else if class == "parent" {
                crate::types::type_map::enclosing_class_at(source, doc, position)
                    .and_then(|enc| find_parent_class_name(&doc.program().stmts, &enc))
                    .unwrap_or_else(|| class.clone())
            } else {
                class.clone()
            };
            for d in all_docs() {
                if let Some((sig, db)) = find_param_sig_in_stmts(
                    d.source(),
                    &d.program().stmts,
                    method,
                    Some(&effective_class),
                    label,
                ) {
                    return Some(format_named_param_hover(&sig, db.as_ref(), label));
                }
            }
            None
        }
    }
}
fn find_param_sig_in_stmts(
    source: &str,
    stmts: &[Stmt<'_, '_>],
    callee_name: &str,
    class_name: Option<&str>,
    label: &str,
) -> Option<(String, Option<crate::lang::docblock::Docblock>)> {
    for stmt in stmts {
        match &stmt.kind {
            StmtKind::Function(f) if class_name.is_none() && f.name == callee_name => {
                let param = f.params.iter().find(|p| p.name == label)?;
                let sig = format_single_param(param);
                let db = crate::lang::docblock::docblock_before(source, stmt.span.start)
                    .map(|raw| crate::lang::docblock::parse_docblock(&raw));
                return Some((sig, db));
            }
            StmtKind::Class(c)
                if class_name
                    .as_ref()
                    .map(|cn| cn == &c.name.as_ref().map(|n| n.to_string()).unwrap_or_default())
                    .unwrap_or(false) =>
            {
                for member in c.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == callee_name
                    {
                        let param = m.params.iter().find(|p| p.name == label)?;
                        let sig = format_single_param(param);
                        let db = crate::lang::docblock::docblock_before(source, member.span.start)
                            .map(|raw| crate::lang::docblock::parse_docblock(&raw));
                        return Some((sig, db));
                    }
                }
            }
            StmtKind::Trait(t)
                if class_name
                    .as_ref()
                    .map(|cn| cn == &t.name.to_string())
                    .unwrap_or(false) =>
            {
                for member in t.body.members.iter() {
                    if let ClassMemberKind::Method(m) = &member.kind
                        && m.name == callee_name
                    {
                        let param = m.params.iter().find(|p| p.name == label)?;
                        let sig = format_single_param(param);
                        let db = crate::lang::docblock::docblock_before(source, member.span.start)
                            .map(|raw| crate::lang::docblock::parse_docblock(&raw));
                        return Some((sig, db));
                    }
                }
            }
            StmtKind::Namespace(ns) => {
                if let NamespaceBody::Braced(inner) = &ns.body
                    && let Some(r) = find_param_sig_in_stmts(
                        source,
                        &inner.stmts,
                        callee_name,
                        class_name,
                        label,
                    )
                {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn format_single_param(p: &Param<'_, '_>) -> String {
    let mut s = String::new();
    if let Some(t) = &p.type_hint {
        s.push_str(&format_type_hint(t));
        s.push(' ');
    }
    if p.variadic {
        s.push_str("...");
    }
    s.push('$');
    s.push_str(&p.name.to_string());
    if let Some(default) = &p.default {
        s.push_str(&format!(" = {}", format_default_value(default)));
    }
    s
}

fn format_named_param_hover(
    sig: &str,
    db: Option<&crate::lang::docblock::Docblock>,
    label: &str,
) -> String {
    let mut value = wrap_php(&format!("(parameter) {}", sig));
    // Include the @param description for this parameter from the docblock.
    if let Some(db) = db {
        let matching_param = db.params.iter().find(|p| {
            p.name == label
                || p.name == format!("${}", label)
                || p.name.trim_start_matches('$') == label
        });
        if let Some(param) = matching_param
            && !param.description.is_empty()
        {
            value.push_str(&format!("\n\n---\n\n{}", param.description));
        }
    }
    value
}
